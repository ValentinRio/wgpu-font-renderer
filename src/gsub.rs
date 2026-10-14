//! Bounded GSUB closure over every lookup, independent of feature selection.
use crate::LoadingError;
use skrifa::{
    raw::{
        tables::gsub::{Gsub, SingleSubst, SubstitutionSubtables},
        ReadError, ResolveOffset,
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

struct Budget {
    remaining: usize,
    invalid: bool,
}
impl Budget {
    fn spend(&mut self, amount: usize) -> Result<()> {
        self.remaining = self
            .remaining
            .checked_sub(amount)
            .ok_or(LoadingError::SubstitutionTooComplex)?;
        Ok(())
    }
}

fn complete<'a, T: skrifa::raw::MinByteRange<'a>>(table: &T, budget: &mut Budget) -> Result<()> {
    let bytes = table.min_table_bytes();
    if bytes.is_empty() {
        return Err(LoadingError::InvalidFont);
    }
    // Includes all declared array elements, before any offset resolution/expansion.
    budget.spend(bytes.len().div_ceil(2))
}

fn coverage(
    table: skrifa::raw::tables::layout::CoverageTable<'_>,
    budget: &mut Budget,
) -> Result<Vec<u16>> {
    use skrifa::raw::tables::layout::CoverageTable;
    complete(&table, budget)?;
    let mut glyphs = Vec::new();
    match table {
        CoverageTable::Format1(table) => {
            for glyph in table.glyph_array() {
                let glyph = glyph.get().to_u16();
                if glyphs.last().is_some_and(|&previous| previous >= glyph) {
                    return Err(LoadingError::InvalidFont);
                }
                glyphs.push(glyph);
            }
        }
        CoverageTable::Format2(table) => {
            for record in table.range_records() {
                let start = record.start_glyph_id().to_u16();
                let end = record.end_glyph_id().to_u16();
                if start > end {
                    // Defer invalidity until the bounded scan finishes: aliased
                    // empty ranges must still pay their declared work cost.
                    budget.invalid = true;
                    continue;
                }
                if record.start_coverage_index() as usize != glyphs.len()
                    || glyphs.last().is_some_and(|&previous| previous >= start)
                {
                    return Err(LoadingError::InvalidFont);
                }
                budget.spend(end as usize - start as usize + 1)?;
                glyphs.extend(start..=end);
            }
        }
    }
    Ok(glyphs)
}

fn lookup_records(
    records: &[skrifa::raw::tables::layout::SequenceLookupRecord],
    count: u16,
    input: u16,
    budget: &mut Budget,
) -> Result<()> {
    budget.spend(records.len())?;
    if input == 0
        || records
            .iter()
            .any(|r| r.lookup_list_index() >= count || r.sequence_index() >= input)
    {
        return Err(LoadingError::InvalidFont);
    }
    Ok(())
}

fn class_selector(
    table: skrifa::raw::tables::layout::ClassDef<'_>,
    glyphs: &[u16],
    count: u16,
    budget: &mut Budget,
) -> Result<()> {
    use skrifa::raw::tables::layout::ClassDef;
    complete(&table, budget)?;
    match &table {
        ClassDef::Format1(t) => {
            if u32::from(t.start_glyph_id().to_u16()) + u32::from(t.glyph_count()) > 65536 {
                return Err(LoadingError::InvalidFont);
            }
        }
        ClassDef::Format2(t) => {
            let mut previous = None;
            for record in t.class_range_records() {
                let start = record.start_glyph_id().to_u16();
                let end = record.end_glyph_id().to_u16();
                if start > end || previous.is_some_and(|last| last >= start) {
                    return Err(LoadingError::InvalidFont);
                }
                previous = Some(end);
            }
        }
    }
    budget.spend(glyphs.len())?;
    if glyphs
        .iter()
        .any(|&gid| table.get(GlyphId::new(gid.into())) >= count)
    {
        return Err(LoadingError::InvalidFont);
    }
    Ok(())
}

fn context(
    table: skrifa::raw::tables::layout::SequenceContext<'_>,
    count: u16,
    budget: &mut Budget,
) -> Result<()> {
    use skrifa::raw::tables::layout::SequenceContext;
    complete(&table, budget)?;
    macro_rules! sets {
        ($sets:expr, $rules:ident) => {
            for set in $sets.iter().flatten() {
                let set = read(set)?;
                complete(&set, budget)?;
                for rule in set.$rules().iter() {
                    let rule = read(rule)?;
                    complete(&rule, budget)?;
                    lookup_records(rule.seq_lookup_records(), count, rule.glyph_count(), budget)?;
                }
            }
        };
    }
    match table {
        SequenceContext::Format1(table) => {
            let glyphs = coverage(read(table.coverage())?, budget)?;
            if glyphs.len() > table.seq_rule_set_count() as usize {
                return Err(LoadingError::InvalidFont);
            }
            sets!(table.seq_rule_sets(), seq_rules);
        }
        SequenceContext::Format2(table) => {
            let glyphs = coverage(read(table.coverage())?, budget)?;
            class_selector(
                read(table.class_def())?,
                &glyphs,
                table.class_seq_rule_set_count(),
                budget,
            )?;
            sets!(table.class_seq_rule_sets(), class_seq_rules);
        }
        SequenceContext::Format3(table) => {
            for cov in table.coverages().iter() {
                coverage(read(cov)?, budget)?;
            }
            lookup_records(
                table.seq_lookup_records(),
                count,
                table.glyph_count(),
                budget,
            )?;
        }
    }
    Ok(())
}

fn chain_context(
    table: skrifa::raw::tables::layout::ChainedSequenceContext<'_>,
    count: u16,
    budget: &mut Budget,
) -> Result<()> {
    use skrifa::raw::tables::layout::ChainedSequenceContext;
    complete(&table, budget)?;
    macro_rules! sets {
        ($sets:expr, $rules:ident) => {
            for set in $sets.iter().flatten() {
                let set = read(set)?;
                complete(&set, budget)?;
                for rule in set.$rules().iter() {
                    let rule = read(rule)?;
                    complete(&rule, budget)?;
                    lookup_records(
                        rule.seq_lookup_records(),
                        count,
                        rule.input_glyph_count(),
                        budget,
                    )?;
                }
            }
        };
    }
    match table {
        ChainedSequenceContext::Format1(table) => {
            let glyphs = coverage(read(table.coverage())?, budget)?;
            if glyphs.len() > table.chained_seq_rule_set_count() as usize {
                return Err(LoadingError::InvalidFont);
            }
            sets!(table.chained_seq_rule_sets(), chained_seq_rules);
        }
        ChainedSequenceContext::Format2(table) => {
            let glyphs = coverage(read(table.coverage())?, budget)?;
            complete(&read(table.backtrack_class_def())?, budget)?;
            class_selector(
                read(table.input_class_def())?,
                &glyphs,
                table.chained_class_seq_rule_set_count(),
                budget,
            )?;
            complete(&read(table.lookahead_class_def())?, budget)?;
            sets!(table.chained_class_seq_rule_sets(), chained_class_seq_rules);
        }
        ChainedSequenceContext::Format3(table) => {
            for cov in table
                .backtrack_coverages()
                .iter()
                .chain(table.input_coverages().iter())
                .chain(table.lookahead_coverages().iter())
            {
                coverage(read(cov)?, budget)?;
            }
            lookup_records(
                table.seq_lookup_records(),
                count,
                table.input_glyph_count(),
                budget,
            )?;
        }
    }
    Ok(())
}

fn feature(
    table: skrifa::raw::tables::layout::Feature<'_>,
    count: u16,
    budget: &mut Budget,
) -> Result<()> {
    complete(&table, budget)?;
    if table
        .lookup_list_indices()
        .iter()
        .any(|index| index.get() >= count)
    {
        return Err(LoadingError::InvalidFont);
    }
    Ok(())
}

fn language(
    table: skrifa::raw::tables::layout::LangSys<'_>,
    count: u16,
    budget: &mut Budget,
) -> Result<()> {
    complete(&table, budget)?;
    let required = table.required_feature_index();
    if (required != u16::MAX && required >= count)
        || table
            .feature_indices()
            .iter()
            .any(|index| index.get() >= count)
    {
        return Err(LoadingError::InvalidFont);
    }
    Ok(())
}

fn scripts(gsub: &Gsub<'_>, count: u16, budget: &mut Budget) -> Result<()> {
    let list = read(gsub.script_list())?;
    complete(&list, budget)?;
    for record in list.script_records() {
        let script = read(record.script(list.offset_data()))?;
        complete(&script, budget)?;
        if let Some(default) = script.default_lang_sys() {
            language(read(default)?, count, budget)?;
        }
        for record in script.lang_sys_records() {
            language(read(record.lang_sys(script.offset_data()))?, count, budget)?;
        }
    }
    Ok(())
}

fn features(gsub: &Gsub<'_>, count: u16, budget: &mut Budget) -> Result<()> {
    let list = read(gsub.feature_list())?;
    complete(&list, budget)?;
    scripts(gsub, list.feature_count(), budget)?;
    for record in list.feature_records() {
        feature(read(record.feature(list.offset_data()))?, count, budget)?;
    }
    if let Some(variations) = gsub.feature_variations() {
        let variations = read(variations)?;
        complete(&variations, budget)?;
        for record in variations.feature_variation_records() {
            if let Some(substitution) = record.feature_table_substitution(variations.offset_data())
            {
                let substitution = read(substitution)?;
                complete(&substitution, budget)?;
                for record in substitution.substitutions() {
                    if record.feature_index() >= list.feature_count() {
                        return Err(LoadingError::InvalidFont);
                    }
                    let alternate = read(record.alternate_feature_offset().resolve_with_args(
                        substitution.offset_data(),
                        &list.feature_records()[record.feature_index() as usize].feature_tag(),
                    ))?;
                    feature(alternate, count, budget)?;
                }
            }
        }
    }
    Ok(())
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
    let mut budget = Budget {
        remaining: MAX_WORK,
        invalid: false,
    };
    let mut rules: Vec<(Vec<u16>, Vec<u16>)> = Vec::new();
    complete(&gsub, &mut budget)?;
    let list = read(gsub.lookup_list())?;
    complete(&list, &mut budget)?;
    features(&gsub, list.lookup_count(), &mut budget)?;
    for lookup in list.lookups().iter() {
        budget.spend(1)?;
        let lookup = read(lookup)?;
        use skrifa::raw::tables::gsub::SubstitutionLookup;
        let subtable_count = match &lookup {
            SubstitutionLookup::Single(t) => {
                complete(t, &mut budget)?;
                t.subtable_offsets().len()
            }
            SubstitutionLookup::Multiple(t) => {
                complete(t, &mut budget)?;
                t.subtable_offsets().len()
            }
            SubstitutionLookup::Alternate(t) => {
                complete(t, &mut budget)?;
                t.subtable_offsets().len()
            }
            SubstitutionLookup::Ligature(t) => {
                complete(t, &mut budget)?;
                t.subtable_offsets().len()
            }
            SubstitutionLookup::Contextual(t) => {
                complete(t, &mut budget)?;
                t.subtable_offsets().len()
            }
            SubstitutionLookup::ChainContextual(t) => {
                complete(t, &mut budget)?;
                t.subtable_offsets().len()
            }
            SubstitutionLookup::Extension(t) => {
                complete(t, &mut budget)?;
                t.subtable_offsets().len()
            }
            SubstitutionLookup::Reverse(t) => {
                complete(t, &mut budget)?;
                t.subtable_offsets().len()
            }
        };
        budget.spend(subtable_count)?;
        if let SubstitutionLookup::Extension(table) = &lookup {
            use skrifa::raw::tables::gsub::ExtensionSubstFormat1;
            let mut kind = None;
            for offset in table.subtable_offsets() {
                let extension: ExtensionSubstFormat1<'_> =
                    read(offset.get().resolve(table.offset_data()))?;
                complete(&extension, &mut budget)?;
                let current = extension.extension_lookup_type();
                if extension.subst_format() != 1
                    || current == 7
                    || !(1..=8).contains(&current)
                    || kind.is_some_and(|previous| previous != current)
                {
                    return Err(LoadingError::InvalidFont);
                }
                kind = Some(current);
            }
        }

        macro_rules! tables {
            ($tables:expr, $table:ident, $body:block) => {
                for table in $tables.iter() {
                    budget.spend(1)?;
                    let $table = read(table)?;
                    complete(&$table, &mut budget)?;
                    $body
                }
            };
        }
        macro_rules! covered {
            ($table:ident, $gid:ident, $index:ident, $body:block) => {
                for ($index, glyph) in coverage(read($table.coverage())?, &mut budget)?
                    .into_iter()
                    .enumerate()
                {
                    budget.spend(1)?;
                    let $gid = glyph;
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
                    complete(&sequence, &mut budget)?;
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
                    complete(&alternates, &mut budget)?;
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
                    complete(&set, &mut budget)?;
                    for ligature in set.ligatures().iter() {
                        budget.spend(1)?;
                        let ligature = read(ligature)?;
                        complete(&ligature, &mut budget)?;
                        if ligature.component_count() == 0 {
                            return Err(LoadingError::InvalidFont);
                        }
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
                for cov in table
                    .backtrack_coverages()
                    .iter()
                    .chain(table.lookahead_coverages().iter())
                {
                    coverage(read(cov)?, &mut budget)?;
                }
                covered!(table, gid, index, {
                    let output = table
                        .substitute_glyph_ids()
                        .get(index)
                        .ok_or(LoadingError::InvalidFont)?;
                    rules.push((vec![gid], vec![output.get().to_u16()]));
                });
            }),
            SubstitutionSubtables::Contextual(tables) => tables!(tables, table, {
                context(table, list.lookup_count(), &mut budget)?;
            }),
            SubstitutionSubtables::ChainContextual(tables) => tables!(tables, table, {
                chain_context(table, list.lookup_count(), &mut budget)?;
            }),
            SubstitutionSubtables::EmptyExtension => return Err(LoadingError::InvalidFont),
        }
    }
    if budget.invalid {
        return Err(LoadingError::InvalidFont);
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
    #[test]
    fn round2_coverage_recorded_index_is_rejected() {
        let data = gsub(&[(1, words(&[2, 10, 2, 3, 4, 2, 1, 1, 1, 1]))]);
        assert!(
            matches!(run(&data, &[1]), Err(LoadingError::InvalidFont)),
            "result: {:?}",
            run(&data, &[1])
        );
    }

    #[test]
    fn round2_truncated_ligature_components_are_rejected() {
        let data = gsub(&[(4, words(&[1, 8, 1, 14, 1, 1, 1, 1, 4, 7, 3]))]);
        assert!(
            matches!(run(&data, &[1]), Err(LoadingError::InvalidFont)),
            "result: {:?}",
            run(&data, &[1])
        );
    }

    #[test]
    fn round2_nested_out_of_range_lookup_is_rejected() {
        let data = gsub(&[(5, words(&[3, 1, 1, 12, 0, 1, 1, 1, 1]))]);
        assert!(
            matches!(run(&data, &[1]), Err(LoadingError::InvalidFont)),
            "result: {:?}",
            run(&data, &[1])
        );
    }

    fn feature_data(index: u16, variation: bool) -> Vec<u8> {
        let mut data = gsub(&[single(1, 2)]);
        let feature_offset = data.len() as u16;
        data[6..8].copy_from_slice(&feature_offset.to_be_bytes());
        data.extend(words(&[
            1,
            0x6c69,
            0x6761,
            8,
            0,
            1,
            if variation { 0 } else { index },
        ]));
        if variation {
            data.splice(10..10, [0; 4]);
            data[2..4].copy_from_slice(&1u16.to_be_bytes());
            for at in [4, 6, 8] {
                let offset = u16::from_be_bytes(data[at..at + 2].try_into().unwrap()) + 4;
                data[at..at + 2].copy_from_slice(&offset.to_be_bytes());
            }
            let offset = data.len() as u32;
            data[10..14].copy_from_slice(&offset.to_be_bytes());
            // FV header and record, then substitution table and alternate feature.
            data.extend(words(&[
                1, 0, 0, 1, 0, 0, 0, 16, 1, 0, 1, 0, 0, 12, 0, 1, index,
            ]));
        }
        data
    }

    #[test]
    fn round2_feature_out_of_range_lookup_is_rejected() {
        let data = feature_data(1, false);
        assert!(
            matches!(run(&data, &[1]), Err(LoadingError::InvalidFont)),
            "result: {:?}",
            run(&data, &[1])
        );
    }

    #[test]
    fn round2_feature_variation_out_of_range_lookup_is_rejected() {
        let data = feature_data(1, true);
        assert!(
            matches!(run(&data, &[1]), Err(LoadingError::InvalidFont)),
            "result: {:?}",
            run(&data, &[1])
        );
    }

    #[test]
    fn round2_reversed_coverage_range_is_rejected() {
        let data = gsub(&[(1, words(&[1, 6, 1, 2, 1, 2, 1, 0]))]);
        assert!(
            matches!(run(&data, &[1]), Err(LoadingError::InvalidFont)),
            "result: {:?}",
            run(&data, &[1])
        );
    }

    #[test]
    fn round2_aliased_empty_ranges_hit_budget_quickly() {
        let mut data = words(&[1, 0, 10, 10, 12, 0, 8]);
        data.extend(words(&[18; 8]));
        data.extend(words(&[1, 0, 100]));
        data.extend(words(&[206; 100]));
        data.extend(words(&[1, 6, 1, 2, 10_000]));
        for _ in 0..10_000 {
            data.extend(words(&[1, 0, 0]));
        }
        let start = std::time::Instant::now();
        let result = run(&data, &[1]);
        assert!(
            matches!(result, Err(LoadingError::SubstitutionTooComplex)),
            "result: {result:?}"
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn round2_script_out_of_range_feature_is_rejected() {
        let mut data = feature_data(0, false);
        let offset = data.len() as u16;
        data[4..6].copy_from_slice(&offset.to_be_bytes());
        data.extend(words(&[1, 0x6c61, 0x746e, 8, 4, 0, 0, 0xffff, 1, 1]));
        assert!(
            matches!(run(&data, &[1]), Err(LoadingError::InvalidFont)),
            "result: {:?}",
            run(&data, &[1])
        );
    }

    #[test]
    fn round2_mixed_extension_types_are_rejected() {
        // One lookup with single and ligature extension headers. read-fonts'
        // convenience iterator assumes the first type for every subtable.
        let mut data = words(&[1, 0, 10, 10, 12, 0, 1, 4, 7, 0, 2, 10, 30]);
        data.extend(words(&[1, 1, 0, 8, 1, 6, 1, 1, 1, 1]));
        data.extend(words(&[1, 4, 0, 8, 1, 8, 1, 14, 1, 1, 1, 1, 4, 7, 2, 2]));
        assert!(
            matches!(run(&data, &[1]), Err(LoadingError::InvalidFont)),
            "result: {:?}",
            run(&data, &[1])
        );
    }

    #[test]
    fn round2_context_formats_reject_out_of_range_targets() {
        for (kind, table) in [
            (5, words(&[1, 8, 1, 14, 1, 1, 1, 1, 4, 1, 1, 0, 1])),
            (
                5,
                words(&[2, 10, 16, 1, 24, 1, 1, 1, 1, 1, 1, 0, 1, 4, 1, 1, 0, 1]),
            ),
            (6, words(&[1, 8, 1, 14, 1, 1, 1, 1, 4, 0, 1, 0, 1, 0, 1])),
            (
                6,
                words(&[
                    2, 14, 20, 20, 20, 1, 28, 1, 1, 1, 1, 1, 1, 0, 1, 4, 0, 1, 0, 1, 0, 1,
                ]),
            ),
            (6, words(&[3, 0, 1, 16, 0, 1, 0, 1, 1, 1, 1])),
        ] {
            let result = run(&gsub(&[(kind, table)]), &[1]);
            assert!(
                matches!(result, Err(LoadingError::InvalidFont)),
                "kind {kind}: {result:?}"
            );
        }
    }
    #[test]
    fn round2_reverse_context_coverage_is_validated() {
        let data = gsub(&[(8, words(&[1, 14, 1, 20, 0, 1, 7, 1, 1, 1, 2, 1, 2, 1, 0]))]);
        let result = run(&data, &[1]);
        assert!(
            matches!(result, Err(LoadingError::InvalidFont)),
            "result: {result:?}"
        );
    }

    #[test]
    fn round2_context_selectors_cannot_escape_declared_rule_sets() {
        for table in [
            words(&[1, 8, 0, 14, 1, 1, 1, 1, 4, 1, 1, 0, 1]),
            words(&[2, 12, 18, 1, 0, 26, 1, 1, 1, 1, 1, 1, 1, 1, 4, 1, 1, 0, 1]),
        ] {
            let result = run(&gsub(&[(5, table)]), &[1]);
            assert!(
                matches!(result, Err(LoadingError::InvalidFont)),
                "result: {result:?}"
            );
        }
    }
}
