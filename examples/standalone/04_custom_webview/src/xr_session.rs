use anyhow::Result;
use ash::vk::Handle;
use openxr as xr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrState {
    Idle,
    Running,
    Stopping,
}

pub struct XrEyeView {
    pub projection_matrix: [f32; 16],
    pub view_matrix: [f32; 16],
}

pub struct XrFrameData {
    pub views: Vec<XrEyeView>,
    pub predicted_display_time: xr::Time,
    pub should_render: bool,
}

pub struct XrSession {
    instance: xr::Instance,
    session: xr::Session<xr::Vulkan>,
    frame_waiter: xr::FrameWaiter,
    frame_stream: xr::FrameStream<xr::Vulkan>,
    swapchain: xr::Swapchain<xr::Vulkan>,
    swapchain_images: Vec<wgpu::Texture>,
    reference_space: xr::Space,
    state: XrState,
    swapchain_width: u32,
    swapchain_height: u32,
}

impl XrSession {
    pub fn new(
        wgpu_instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
    ) -> Result<Self> {
        let (vk_instance_raw, vk_phys_dev_raw, vk_device_raw, queue_family_index) =
            extract_vulkan_handles(wgpu_instance, adapter, device)?;

        let xr_entry = unsafe { xr::Entry::load()? };

        #[cfg(target_os = "android")]
        xr_entry.initialize_android_loader()?;

        let available_extensions = xr_entry.enumerate_extensions()?;
        log::info!(
            "OpenXR available: khr_vulkan_enable={}, khr_vulkan_enable2={}",
            available_extensions.khr_vulkan_enable,
            available_extensions.khr_vulkan_enable2
        );

        let mut enabled_extensions = xr::ExtensionSet::default();
        enabled_extensions.khr_vulkan_enable = true;
        #[cfg(target_os = "android")]
        {
            enabled_extensions.khr_android_create_instance = true;
        }

        let xr_instance = xr_entry.create_instance(
            &xr::ApplicationInfo {
                application_name: "CustomWebview",
                application_version: 1,
                engine_name: "wgpu",
                engine_version: 1,
                api_version: xr::Version::new(1, 0, 0),
            },
            &enabled_extensions,
            &[],
        )?;

        let system = xr_instance.system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)?;
        let _reqs = xr_instance.graphics_requirements::<xr::Vulkan>(system)?;

        let xr_phys_dev = unsafe {
            xr_instance.vulkan_graphics_device(system, vk_instance_raw as _)?
        };
        log::info!(
            "XR physical device: {:?}, wgpu physical device: {:?}",
            xr_phys_dev,
            vk_phys_dev_raw
        );

        let binding = xr::vulkan::SessionCreateInfo {
            instance: vk_instance_raw as _,
            physical_device: xr_phys_dev as _,
            device: vk_device_raw as _,
            queue_family_index,
            queue_index: 0,
        };

        let (session, frame_waiter, frame_stream) =
            unsafe { xr_instance.create_session::<xr::Vulkan>(system, &binding)? };

        let view_configs = xr_instance.enumerate_view_configuration_views(
            system,
            xr::ViewConfigurationType::PRIMARY_STEREO,
        )?;
        let width = view_configs[0].recommended_image_rect_width;
        let height = view_configs[0].recommended_image_rect_height;
        log::info!("XR swapchain size: {}x{}", width, height);

        let swapchain = session.create_swapchain(&xr::SwapchainCreateInfo {
            create_flags: xr::SwapchainCreateFlags::EMPTY,
            usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                | xr::SwapchainUsageFlags::SAMPLED,
            format: ash::vk::Format::R8G8B8A8_SRGB.as_raw() as u32,
            sample_count: 1,
            width,
            height,
            face_count: 1,
            array_size: 2,
            mip_count: 1,
        })?;

        let xr_images = swapchain.enumerate_images()?;
        let swapchain_images = wrap_xr_images_as_wgpu(device, &xr_images, width, height);

        let reference_space =
            session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?;

        Ok(Self {
            instance: xr_instance,
            session,
            frame_waiter,
            frame_stream,
            swapchain,
            swapchain_images,
            reference_space,
            state: XrState::Idle,
            swapchain_width: width,
            swapchain_height: height,
        })
    }

    pub fn state(&self) -> XrState {
        self.state
    }

    pub fn swapchain_size(&self) -> (u32, u32) {
        (self.swapchain_width, self.swapchain_height)
    }

    pub fn poll_events(&mut self) -> Result<()> {
        let mut event_buf = xr::EventDataBuffer::new();
        while let Some(event) = self.instance.poll_event(&mut event_buf)? {
            if let xr::Event::SessionStateChanged(e) = event {
                log::info!("XR session state: {:?}", e.state());
                match e.state() {
                    xr::SessionState::READY => {
                        self.session
                            .begin(xr::ViewConfigurationType::PRIMARY_STEREO)?;
                        self.state = XrState::Running;
                    }
                    xr::SessionState::STOPPING => {
                        self.session.end()?;
                        self.state = XrState::Stopping;
                    }
                    xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                        self.state = XrState::Idle;
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub fn wait_frame(&mut self) -> Result<Option<XrFrameData>> {
        if self.state != XrState::Running {
            return Ok(None);
        }

        let frame_state = self.frame_waiter.wait()?;
        self.frame_stream.begin()?;

        if !frame_state.should_render {
            self.frame_stream.end(
                frame_state.predicted_display_time,
                xr::EnvironmentBlendMode::OPAQUE,
                &[],
            )?;
            return Ok(None);
        }

        let (_, views) = self.session.locate_views(
            xr::ViewConfigurationType::PRIMARY_STEREO,
            frame_state.predicted_display_time,
            &self.reference_space,
        )?;

        let eye_views: Vec<XrEyeView> = views
            .iter()
            .map(|view| XrEyeView {
                projection_matrix: fov_to_projection_matrix(&view.fov, 0.01, 100.0),
                view_matrix: pose_to_view_matrix(&view.pose),
            })
            .collect();

        Ok(Some(XrFrameData {
            views: eye_views,
            predicted_display_time: frame_state.predicted_display_time,
            should_render: true,
        }))
    }

    pub fn acquire_swapchain_image(&mut self) -> Result<(&wgpu::Texture, u32)> {
        let index = self.swapchain.acquire_image()?;
        self.swapchain.wait_image(xr::Duration::INFINITE)?;
        Ok((&self.swapchain_images[index as usize], index))
    }

    pub fn release_and_end_frame(&mut self, frame_data: &XrFrameData) -> Result<()> {
        self.swapchain.release_image()?;

        let rect = xr::Rect2Di {
            offset: xr::Offset2Di { x: 0, y: 0 },
            extent: xr::Extent2Di {
                width: self.swapchain_width as i32,
                height: self.swapchain_height as i32,
            },
        };

        let sub_image_left = xr::SwapchainSubImage::new()
            .swapchain(&self.swapchain)
            .image_array_index(0)
            .image_rect(rect);

        let sub_image_right = xr::SwapchainSubImage::new()
            .swapchain(&self.swapchain)
            .image_array_index(1)
            .image_rect(rect);

        let left_view = &frame_data.views[0];
        let right_view = &frame_data.views[1];

        let left_fov = projection_matrix_to_fov(&left_view.projection_matrix);
        let right_fov = projection_matrix_to_fov(&right_view.projection_matrix);
        let left_pose = view_matrix_to_pose(&left_view.view_matrix);
        let right_pose = view_matrix_to_pose(&right_view.view_matrix);

        let projection_views = [
            xr::CompositionLayerProjectionView::new()
                .pose(left_pose)
                .fov(left_fov)
                .sub_image(sub_image_left),
            xr::CompositionLayerProjectionView::new()
                .pose(right_pose)
                .fov(right_fov)
                .sub_image(sub_image_right),
        ];

        let projection = xr::CompositionLayerProjection::new()
            .space(&self.reference_space)
            .views(&projection_views);

        self.frame_stream.end(
            frame_data.predicted_display_time,
            xr::EnvironmentBlendMode::OPAQUE,
            &[&projection],
        )?;

        Ok(())
    }
}

fn wrap_xr_images_as_wgpu(
    device: &wgpu::Device,
    xr_images: &[u64],
    width: u32,
    height: u32,
) -> Vec<wgpu::Texture> {
    use wgpu::hal;

    xr_images
        .iter()
        .map(|&raw_image| {
            let vk_image = ash::vk::Image::from_raw(raw_image);
            let hal_texture = unsafe {
                let hal_device = device
                    .as_hal::<hal::api::Vulkan>()
                    .expect("not a Vulkan device");
                hal_device.texture_from_raw(
                    vk_image,
                    &hal::TextureDescriptor {
                        label: Some("xr swapchain"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 2,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        usage: wgpu::TextureUses::COLOR_TARGET | wgpu::TextureUses::RESOURCE,
                        memory_flags: hal::MemoryFlags::empty(),
                        view_formats: vec![],
                    },
                    None,
                    hal::vulkan::TextureMemory::External,
                )
            };
            unsafe {
                device.create_texture_from_hal::<hal::api::Vulkan>(
                    hal_texture,
                    &wgpu::TextureDescriptor {
                        label: Some("xr swapchain"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 2,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                )
            }
        })
        .collect()
}

fn extract_vulkan_handles(
    instance: &wgpu::Instance,
    adapter: &wgpu::Adapter,
    device: &wgpu::Device,
) -> Result<(u64, u64, u64, u32)> {
    use wgpu::hal;

    let vk_instance = unsafe {
        instance
            .as_hal::<hal::api::Vulkan>()
            .map(|inst| inst.shared_instance().raw_instance().handle().as_raw())
            .ok_or_else(|| anyhow::anyhow!("not a Vulkan instance"))?
    };

    let vk_physical_device = unsafe {
        adapter
            .as_hal::<hal::api::Vulkan>()
            .map(|a| a.raw_physical_device().as_raw())
            .ok_or_else(|| anyhow::anyhow!("not a Vulkan adapter"))?
    };

    let (vk_device, queue_family) = unsafe {
        device
            .as_hal::<hal::api::Vulkan>()
            .map(|d| {
                let raw = d.raw_device().handle().as_raw();
                let qf = d.queue_family_index();
                (raw, qf)
            })
            .ok_or_else(|| anyhow::anyhow!("not a Vulkan device"))?
    };

    Ok((vk_instance, vk_physical_device, vk_device, queue_family))
}

fn fov_to_projection_matrix(fov: &xr::Fovf, near: f32, far: f32) -> [f32; 16] {
    let left = fov.angle_left.tan();
    let right = fov.angle_right.tan();
    let up = fov.angle_up.tan();
    let down = fov.angle_down.tan();

    let w = right - left;
    let h = up - down;

    #[rustfmt::skip]
    let m = [
        2.0 / w,            0.0,                0.0,                             0.0,
        0.0,                2.0 / h,            0.0,                             0.0,
        (right + left) / w, (up + down) / h,    -(far + near) / (far - near),   -1.0,
        0.0,                0.0,                -2.0 * far * near / (far - near), 0.0,
    ];
    m
}

fn pose_to_view_matrix(pose: &xr::Posef) -> [f32; 16] {
    let q = &pose.orientation;
    let t = &pose.position;

    let x2 = q.x * 2.0;
    let y2 = q.y * 2.0;
    let z2 = q.z * 2.0;
    let xx = q.x * x2;
    let xy = q.x * y2;
    let xz = q.x * z2;
    let yy = q.y * y2;
    let yz = q.y * z2;
    let zz = q.z * z2;
    let wx = q.w * x2;
    let wy = q.w * y2;
    let wz = q.w * z2;

    // View = inverse(pose) = transpose(R) * translate(-t)
    let r00 = 1.0 - (yy + zz);
    let r01 = xy + wz;
    let r02 = xz - wy;
    let r10 = xy - wz;
    let r11 = 1.0 - (xx + zz);
    let r12 = yz + wx;
    let r20 = xz + wy;
    let r21 = yz - wx;
    let r22 = 1.0 - (xx + yy);

    let tx = -(r00 * t.x + r01 * t.y + r02 * t.z);
    let ty = -(r10 * t.x + r11 * t.y + r12 * t.z);
    let tz = -(r20 * t.x + r21 * t.y + r22 * t.z);

    #[rustfmt::skip]
    let m = [
        r00, r10, r20, 0.0,
        r01, r11, r21, 0.0,
        r02, r12, r22, 0.0,
        tx,  ty,  tz,  1.0,
    ];
    m
}

fn projection_matrix_to_fov(m: &[f32; 16]) -> xr::Fovf {
    let w = 2.0 / m[0];
    let h = 2.0 / m[5];
    let rpl = m[8] * w;
    let upd = m[9] * h;

    let right = (w + rpl) / 2.0;
    let left = right - w;
    let up = (h + upd) / 2.0;
    let down = up - h;

    xr::Fovf {
        angle_left: left.atan(),
        angle_right: right.atan(),
        angle_up: up.atan(),
        angle_down: down.atan(),
    }
}

fn view_matrix_to_pose(m: &[f32; 16]) -> xr::Posef {
    let r00 = m[0];
    let r10 = m[1];
    let r20 = m[2];
    let r01 = m[4];
    let r11 = m[5];
    let r21 = m[6];
    let r02 = m[8];
    let r12 = m[9];
    let r22 = m[10];

    let trace = r00 + r11 + r22;
    let (w, x, y, z) = if trace > 0.0 {
        let s = 0.5 / (trace + 1.0).sqrt();
        (0.25 / s, (r21 - r12) * s, (r02 - r20) * s, (r10 - r01) * s)
    } else if r00 > r11 && r00 > r22 {
        let s = 2.0 * (1.0 + r00 - r11 - r22).sqrt();
        ((r21 - r12) / s, 0.25 * s, (r01 + r10) / s, (r02 + r20) / s)
    } else if r11 > r22 {
        let s = 2.0 * (1.0 + r11 - r00 - r22).sqrt();
        ((r02 - r20) / s, (r01 + r10) / s, 0.25 * s, (r12 + r21) / s)
    } else {
        let s = 2.0 * (1.0 + r22 - r00 - r11).sqrt();
        ((r10 - r01) / s, (r02 + r20) / s, (r12 + r21) / s, 0.25 * s)
    };

    let tx = -(r00 * m[12] + r10 * m[13] + r20 * m[14]);
    let ty = -(r01 * m[12] + r11 * m[13] + r21 * m[14]);
    let tz = -(r02 * m[12] + r12 * m[13] + r22 * m[14]);

    xr::Posef {
        orientation: xr::Quaternionf { x, y, z, w },
        position: xr::Vector3f {
            x: tx,
            y: ty,
            z: tz,
        },
    }
}
