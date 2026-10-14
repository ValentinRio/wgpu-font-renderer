//! Bounded GSUB closure over every lookup, independent of feature selection.
use crate::LoadingError;
use skrifa::{
    raw::{
        tables::gsub::{Gsub, SingleSubst, SubstitutionSubtables},
        ReadError,
    },
    GlyphId,
};
use std::collections::BTreeSet;

const MAX_WORK: usize = 4_000_000;
const MAX_ITERATIONS: usize = 128;

type Result<T> = std::result::Result<T, LoadingError>;

fn read<T>(value: std::result::Result<T, ReadError>) -> Result<T> {
    value.map_err(|_| LoadingError::InvalidFont)
}

struct Budget(usize);
impl Budget {
    fn spend(&mut self, amount: usize) -> Result<()> {
        self.0 = self
            .0
            .checked_sub(amount)
            .ok_or(LoadingError::SubstitutionTooComplex)?;
        Ok(())
    }
}

// Compile substitutions once. Context/chained context only invoke other lookups;
// visiting *every* lookup already includes those targets, without recursive work.
// Their context constraints (and reverse chaining's context) are intentionally
// ignored: this is an over-approximation across scripts, features and variations.
pub(crate) fn closure(
    gsub: Option<Gsub<'_>>,
    seeds: impl Iterator<Item = GlyphId>,
) -> Result<Vec<GlyphId>> {
    let seeds: Vec<_> = seeds.collect();
    let Some(gsub) = gsub else {
        return Ok(seeds);
    };
    let mut glyphs: BTreeSet<u16> = seeds.iter().map(|id| id.to_u32() as u16).collect();
    let mut budget = Budget(MAX_WORK);
    let mut rules: Vec<(Vec<u16>, Vec<u16>)> = Vec::new();
    let list = read(gsub.lookup_list())?;
    for lookup in list.lookups().iter() {
        budget.spend(1)?;
        let lookup = read(lookup)?;
        use skrifa::raw::tables::gsub::SubstitutionLookup;
        let subtable_count = match &lookup {
            SubstitutionLookup::Single(t) => t.subtable_offsets().len(),
            SubstitutionLookup::Multiple(t) => t.subtable_offsets().len(),
            SubstitutionLookup::Alternate(t) => t.subtable_offsets().len(),
            SubstitutionLookup::Ligature(t) => t.subtable_offsets().len(),
            SubstitutionLookup::Contextual(t) => t.subtable_offsets().len(),
            SubstitutionLookup::ChainContextual(t) => t.subtable_offsets().len(),
            SubstitutionLookup::Extension(t) => t.subtable_offsets().len(),
            SubstitutionLookup::Reverse(t) => t.subtable_offsets().len(),
        };
        budget.spend(subtable_count)?;
        macro_rules! tables {
            ($tables:expr, $table:ident, $body:block) => {
                for table in $tables.iter() {
                    budget.spend(1)?;
                    let $table = read(table)?;
                    $body
                }
            };
        }
        macro_rules! covered {
            ($table:ident, $gid:ident, $index:ident, $body:block) => {
                for ($index, glyph) in read($table.coverage())?.iter().enumerate() {
                    budget.spend(1)?;
                    let $gid = glyph.to_u16();
                    $body
                }
            };
        }
        match read(lookup.subtables())? {
            SubstitutionSubtables::Single(tables) => tables!(tables, table, {
                match table {
                    SingleSubst::Format1(table) => covered!(table, gid, _index, {
                        rules.push((
                            vec![gid],
                            vec![gid.wrapping_add(table.delta_glyph_id() as u16)],
                        ));
                    }),
                    SingleSubst::Format2(table) => covered!(table, gid, index, {
                        let output = table
                            .substitute_glyph_ids()
                            .get(index)
                            .ok_or(LoadingError::InvalidFont)?;
                        rules.push((vec![gid], vec![output.get().to_u16()]));
                    }),
                }
            }),
            SubstitutionSubtables::Multiple(tables) => tables!(tables, table, {
                covered!(table, gid, index, {
                    let sequence = read(table.sequences().get(index))?;
                    budget.spend(sequence.substitute_glyph_ids().len())?;
                    rules.push((
                        vec![gid],
                        sequence
                            .substitute_glyph_ids()
                            .iter()
                            .map(|g| g.get().to_u16())
                            .collect(),
                    ));
                });
            }),
            SubstitutionSubtables::Alternate(tables) => tables!(tables, table, {
                covered!(table, gid, index, {
                    let alternates = read(table.alternate_sets().get(index))?;
                    budget.spend(alternates.alternate_glyph_ids().len())?;
                    rules.push((
                        vec![gid],
                        alternates
                            .alternate_glyph_ids()
                            .iter()
                            .map(|g| g.get().to_u16())
                            .collect(),
                    ));
                });
            }),
            SubstitutionSubtables::Ligature(tables) => tables!(tables, table, {
                covered!(table, gid, index, {
                    let set = read(table.ligature_sets().get(index))?;
                    for ligature in set.ligatures().iter() {
                        budget.spend(1)?;
                        let ligature = read(ligature)?;
                        budget.spend(ligature.component_glyph_ids().len())?;
                        let mut inputs = vec![gid];
                        inputs.extend(
                            ligature
                                .component_glyph_ids()
                                .iter()
                                .map(|g| g.get().to_u16()),
                        );
                        rules.push((inputs, vec![ligature.ligature_glyph().to_u16()]));
                    }
                });
            }),
            SubstitutionSubtables::Reverse(tables) => tables!(tables, table, {
                covered!(table, gid, index, {
                    let output = table
                        .substitute_glyph_ids()
                        .get(index)
                        .ok_or(LoadingError::InvalidFont)?;
                    rules.push((vec![gid], vec![output.get().to_u16()]));
                });
            }),
            SubstitutionSubtables::Contextual(_)
            | SubstitutionSubtables::ChainContextual(_)
            | SubstitutionSubtables::EmptyExtension => {}
        }
    }
    for _ in 0..MAX_ITERATIONS {
        let before = glyphs.len();
        for (inputs, outputs) in &rules {
            budget.spend(1 + inputs.len() + outputs.len())?;
            if inputs.iter().all(|g| glyphs.contains(g)) {
                glyphs.extend(outputs);
            }
        }
        if glyphs.len() == before {
            // Preserve preset allocation order; append only new closure members.
            let mut result = seeds;
            for seed in &result {
                glyphs.remove(&(seed.to_u32() as u16));
            }
            result.extend(glyphs.into_iter().map(|id| GlyphId::new(id.into())));
            return Ok(result);
        }
    }
    Err(LoadingError::SubstitutionTooComplex)
}

#[cfg(test)]
mod tests {
    use super::*;
    use skrifa::raw::{FontData, FontRead};

    fn words(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    fn gsub(lookups: &[(u16, Vec<u8>)]) -> Vec<u8> {
        // Script/feature lists are empty: closure must still visit every lookup.
        let mut data = words(&[1, 0, 10, 10, 12, 0, lookups.len() as u16]);
        let mut offset = 2 + 2 * lookups.len();
        for (_, subtable) in lookups {
            data.extend(words(&[offset as u16]));
            offset += 8 + subtable.len();
        }
        for (kind, subtable) in lookups {
            data.extend(words(&[*kind, 0, 1, 8]));
            data.extend(subtable);
        }
        data
    }

    fn run(bytes: &[u8], seeds: &[u16]) -> Result<Vec<u16>> {
        closure(
            Some(Gsub::read(FontData::new(bytes)).unwrap()),
            seeds.iter().map(|&id| GlyphId::new(id.into())),
        )
        .map(|ids| ids.iter().map(|id| id.to_u32() as u16).collect())
    }

    fn single(input: u16, output: u16) -> (u16, Vec<u8>) {
        (1, words(&[2, 8, 1, output, 1, 1, input]))
    }

    #[test]
    fn single_cycles_converge_but_long_cycles_hit_iteration_cap() {
        assert_eq!(
            run(&gsub(&[single(1, 2), single(2, 1)]), &[1]).unwrap(),
            [1, 2]
        );
        // Reverse order forces one new member per pass; the final edge cycles.
        let mut lookups = vec![single(130, 1)];
        lookups.extend((1..130).rev().map(|id| single(id, id + 1)));
        assert!(matches!(
            run(&gsub(&lookups), &[1]),
            Err(LoadingError::SubstitutionTooComplex)
        ));
    }

    #[test]
    fn large_coverage_hits_total_work_cap() {
        // 65 shared-format lookups, each covering the entire glyph-ID domain.
        let lookup = (1, words(&[1, 6, 1, 2, 1, 0, u16::MAX, 0]));
        assert!(matches!(
            run(&gsub(&vec![lookup; 65]), &[1]),
            Err(LoadingError::SubstitutionTooComplex)
        ));
    }

    #[test]
    fn every_output_lookup_and_all_ligature_components() {
        let multiple = (2, words(&[1, 8, 1, 14, 1, 1, 2, 2, 3, 4]));
        let alternate = (3, words(&[1, 8, 1, 14, 1, 1, 4, 2, 5, 6]));
        let ligature = (4, words(&[1, 8, 1, 14, 1, 1, 6, 1, 4, 7, 2, 5]));
        let reverse = (8, words(&[1, 12, 0, 0, 1, 8, 1, 1, 7]));
        // Extension to single format 1: 8 -> 9.
        let extension = (7, words(&[1, 1, 0, 8, 1, 6, 1, 1, 1, 8]));
        // Context and chained context refer to lookup zero. It is visited even
        // with an empty context coverage, as allowed by over-approximation.
        let context = (5, words(&[3, 1, 1, 12, 0, 0, 1, 0]));
        let chain = (6, words(&[3, 0, 1, 16, 0, 1, 0, 0, 1, 0]));
        let data = gsub(&[
            single(1, 2),
            multiple,
            alternate,
            ligature.clone(),
            reverse,
            extension,
            context,
            chain,
        ]);
        assert_eq!(run(&data, &[1]).unwrap(), (1..=9).collect::<Vec<_>>());
        let data = gsub(&[ligature]);
        assert_eq!(run(&data, &[6]).unwrap(), [6]);
        assert_eq!(run(&data, &[5, 6]).unwrap(), [5, 6, 7]);
    }
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn loader_returns_closure_cap_error_before_upload() {
        let Some((device, queue, _)) = crate::atlas_tests::common::gpu() else {
            return;
        };
        let original = include_bytes!("../fonts/Roboto-Regular.ttf");
        let font = swash::FontRef::from_index(original, 0).unwrap();
        let first = font.charmap().map('A');
        // A reversed chain closed into a cycle, starting at the preset glyph.
        let mut lookups = vec![single(first + 130, first)];
        lookups.extend((first..first + 130).rev().map(|id| single(id, id + 1)));
        let mut store = crate::FontStore::new(&device);
        let prior = store
            .load_from_bytes(&device, &queue, original, "A")
            .unwrap();
        for data in [
            gsub(&lookups),
            gsub(&vec![(1, words(&[1, 6, 1, 2, 1, 0, u16::MAX, 0])); 65]),
        ] {
            let mut bytes = original.to_vec();
            let table_count = u16::from_be_bytes(bytes[4..6].try_into().unwrap()) as usize;
            let record = (0..table_count)
                .map(|i| 12 + i * 16)
                .find(|&i| &bytes[i..i + 4] == b"GSUB")
                .unwrap();
            let offset = bytes.len() as u32;
            bytes[record + 8..record + 12].copy_from_slice(&offset.to_be_bytes());
            bytes[record + 12..record + 16].copy_from_slice(&(data.len() as u32).to_be_bytes());
            bytes.extend(data);
            assert!(matches!(
                store.load_from_bytes(&device, &queue, &bytes, "A"),
                Err(LoadingError::SubstitutionTooComplex)
            ));
            assert!(store.get(prior).is_some());
        }
    }
}
