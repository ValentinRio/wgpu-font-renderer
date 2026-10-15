pub fn select(instance: &wgpu::Instance) -> Result<wgpu::Adapter, Box<dyn std::error::Error>> {
    let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
    for adapter in &adapters {
        let info = adapter.get_info();
        println!(
            "Available adapter: {} (backend={:?}, device_type={:?})",
            info.name, info.backend, info.device_type
        );
    }

    let adapter = if let Ok(name) = std::env::var("WGPU_ADAPTER_NAME") {
        // Match wgpu::util's case-insensitive substring rule, but return an error
        // instead of its panic when no adapter matches.
        let desired = name.to_lowercase();
        adapters
            .into_iter()
            .find(|adapter| adapter.get_info().name.to_lowercase().contains(&desired))
            .ok_or_else(|| format!("WGPU_ADAPTER_NAME={name:?}: no matching adapter found"))?
    } else {
        let power_preference =
            wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance);
        println!("Adapter power preference: {power_preference:?}");
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference,
            ..Default::default()
        }))?
    };
    let info = adapter.get_info();
    println!(
        "Chosen adapter: {} (backend={:?}, device_type={:?})",
        info.name, info.backend, info.device_type
    );
    Ok(adapter)
}
