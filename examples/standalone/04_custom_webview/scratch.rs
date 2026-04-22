use anyhow::Result;

fn get_adapter(instance: &wgpu::Instance, target_pd: u64) -> Result<wgpu::Adapter> {
    // just a pseudo check
    Ok(instance.enumerate_adapters(wgpu::Backends::VULKAN).into_iter().next().unwrap())
}
fn main() {}
