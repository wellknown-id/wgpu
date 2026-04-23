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
    pub fov: xr::Fovf,
    pub pose: xr::Posef,
}

pub struct XrFrameData {
    pub views: Vec<XrEyeView>,
    pub predicted_display_time: xr::Time,
    pub should_render: bool,
}

pub struct XrContext {
    pub instance: xr::Instance,
    pub system: xr::SystemId,
}

impl XrContext {
    pub fn new() -> Result<Self> {
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

        Ok(Self {
            instance: xr_instance,
            system,
        })
    }

    pub fn vulkan_graphics_device(&self, wgpu_instance: &wgpu::Instance) -> Result<u64> {
        let vk_instance_raw = extract_vulkan_instance(wgpu_instance)?;
        let xr_phys_dev = unsafe {
            self.instance
                .vulkan_graphics_device(self.system, vk_instance_raw as _)?
        };
        Ok(xr_phys_dev as _)
    }
}

pub fn extract_vulkan_instance(instance: &wgpu::Instance) -> Result<u64> {
    use wgpu::hal;
    unsafe {
        instance
            .as_hal::<hal::api::Vulkan>()
            .map(|inst| inst.shared_instance().raw_instance().handle().as_raw())
            .ok_or_else(|| anyhow::anyhow!("not a Vulkan instance"))
    }
}

pub fn extract_vulkan_physical_device(adapter: &wgpu::Adapter) -> Result<u64> {
    use wgpu::hal;
    unsafe {
        adapter
            .as_hal::<hal::api::Vulkan>()
            .map(|a| a.raw_physical_device().as_raw())
            .ok_or_else(|| anyhow::anyhow!("not a Vulkan adapter"))
    }
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
    action_set: xr::ActionSet,
    select_action: xr::Action<bool>,
    exit_action: xr::Action<bool>,
    right_thumbstick_action: xr::Action<xr::Vector2f>,
    left_thumbstick_action: xr::Action<xr::Vector2f>,
    right_aim_space: xr::Space,
    left_aim_space: xr::Space,
}

impl XrSession {
    pub fn new(
        ctx: &XrContext,
        wgpu_instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
    ) -> Result<Self> {
        let (vk_instance_raw, vk_phys_dev_raw, vk_device_raw, queue_family_index) =
            extract_vulkan_handles(wgpu_instance, adapter, device)?;

        let xr_phys_dev = unsafe {
            ctx.instance
                .vulkan_graphics_device(ctx.system, vk_instance_raw as _)?
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

        let (session, frame_waiter, frame_stream) = unsafe {
            ctx.instance
                .create_session::<xr::Vulkan>(ctx.system, &binding)?
        };

        let view_configs = ctx.instance.enumerate_view_configuration_views(
            ctx.system,
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

        let action_set = ctx.instance.create_action_set("input", "Input", 0)?;
        let select_action = action_set.create_action::<bool>("select", "Select", &[])?;
        let exit_action = action_set.create_action::<bool>("exit", "Exit", &[])?;
        let right_thumbstick_action = action_set.create_action::<xr::Vector2f>(
            "right_thumbstick",
            "Right Thumbstick",
            &[],
        )?;
        let left_thumbstick_action =
            action_set.create_action::<xr::Vector2f>("left_thumbstick", "Left Thumbstick", &[])?;
        let right_aim_action =
            action_set.create_action::<xr::Posef>("right_aim", "Right Aim", &[])?;
        let left_aim_action = action_set.create_action::<xr::Posef>("left_aim", "Left Aim", &[])?;
        let left_select = ctx
            .instance
            .string_to_path("/user/hand/left/input/select/click")?;
        let right_select = ctx
            .instance
            .string_to_path("/user/hand/right/input/select/click")?;
        let left_x = ctx
            .instance
            .string_to_path("/user/hand/left/input/x/click")?;
        let right_a = ctx
            .instance
            .string_to_path("/user/hand/right/input/a/click")?;
        ctx.instance.suggest_interaction_profile_bindings(
            ctx.instance
                .string_to_path("/interaction_profiles/khr/simple_controller")?,
            &[
                xr::Binding::new(&select_action, left_select),
                xr::Binding::new(&select_action, right_select),
                xr::Binding::new(
                    &left_aim_action,
                    ctx.instance
                        .string_to_path("/user/hand/left/input/grip/pose")?,
                ),
                xr::Binding::new(
                    &right_aim_action,
                    ctx.instance
                        .string_to_path("/user/hand/right/input/grip/pose")?,
                ),
            ],
        )?;
        let left_aim = ctx
            .instance
            .string_to_path("/user/hand/left/input/aim/pose")?;
        let right_aim = ctx
            .instance
            .string_to_path("/user/hand/right/input/aim/pose")?;
        let left_thumbstick = ctx
            .instance
            .string_to_path("/user/hand/left/input/thumbstick")?;
        let right_thumbstick = ctx
            .instance
            .string_to_path("/user/hand/right/input/thumbstick")?;
        let left_exit = ctx
            .instance
            .string_to_path("/user/hand/left/input/y/click")?;
        let right_exit = ctx
            .instance
            .string_to_path("/user/hand/right/input/b/click")?;
        for profile_path in [
            "/interaction_profiles/oculus/touch_controller",
            "/interaction_profiles/meta/touch_controller_plus",
        ] {
            if let Ok(profile) = ctx.instance.string_to_path(profile_path) {
                if let Err(error) = ctx.instance.suggest_interaction_profile_bindings(
                    profile,
                    &[
                        xr::Binding::new(&select_action, left_x),
                        xr::Binding::new(&select_action, right_a),
                        xr::Binding::new(&exit_action, left_exit),
                        xr::Binding::new(&exit_action, right_exit),
                        xr::Binding::new(&left_thumbstick_action, left_thumbstick),
                        xr::Binding::new(&right_thumbstick_action, right_thumbstick),
                        xr::Binding::new(&left_aim_action, left_aim),
                        xr::Binding::new(&right_aim_action, right_aim),
                    ],
                ) {
                    log::warn!("Skipping unsupported input profile {profile_path}: {error}");
                }
            }
        }
        session.attach_action_sets(&[&action_set])?;
        let right_aim_space =
            right_aim_action.create_space(&session, xr::Path::NULL, xr::Posef::IDENTITY)?;
        let left_aim_space =
            left_aim_action.create_space(&session, xr::Path::NULL, xr::Posef::IDENTITY)?;

        Ok(Self {
            instance: ctx.instance.clone(),
            session,
            frame_waiter,
            frame_stream,
            swapchain,
            swapchain_images,
            reference_space,
            state: XrState::Idle,
            swapchain_width: width,
            swapchain_height: height,
            action_set,
            select_action,
            exit_action,
            right_thumbstick_action,
            left_thumbstick_action,
            right_aim_space,
            left_aim_space,
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
        if views.len() < 2 {
            log::warn!(
                "XR locate_views returned {} view(s); expected stereo. Skipping frame.",
                views.len()
            );
            self.frame_stream.end(
                frame_state.predicted_display_time,
                xr::EnvironmentBlendMode::OPAQUE,
                &[],
            )?;
            return Ok(None);
        }

        for view in views.iter().take(2) {
            if !is_valid_pose(&view.pose) {
                log::warn!("XR locate_views returned an invalid view pose; skipping frame.");
                self.frame_stream.end(
                    frame_state.predicted_display_time,
                    xr::EnvironmentBlendMode::OPAQUE,
                    &[],
                )?;
                return Ok(None);
            }
        }

        let eye_views: Vec<XrEyeView> = views
            .iter()
            .map(|view| XrEyeView {
                projection_matrix: fov_to_projection_matrix(&view.fov, 0.01, 100.0),
                view_matrix: pose_to_view_matrix(&view.pose),
                fov: view.fov,
                pose: view.pose,
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

    pub fn end_frame_without_layers(&mut self, predicted_display_time: xr::Time) -> Result<()> {
        self.frame_stream.end(
            predicted_display_time,
            xr::EnvironmentBlendMode::OPAQUE,
            &[],
        )?;
        Ok(())
    }

    pub fn sync_input(&self) -> Result<()> {
        self.session.sync_actions(&[(&self.action_set).into()])?;
        Ok(())
    }

    pub fn select_pressed(&self) -> Result<bool> {
        let select_state = self.select_action.state(&self.session, xr::Path::NULL)?;
        Ok(select_state.is_active && select_state.current_state)
    }

    pub fn exit_pressed(&self) -> Result<bool> {
        let exit_state = self.exit_action.state(&self.session, xr::Path::NULL)?;
        Ok(exit_state.is_active && exit_state.current_state)
    }

    pub fn pointer_pose(&self, predicted_display_time: xr::Time) -> Result<Option<xr::Posef>> {
        for space in [&self.right_aim_space, &self.left_aim_space] {
            let location = space.locate(&self.reference_space, predicted_display_time)?;
            let flags = location.location_flags;
            if flags.contains(xr::SpaceLocationFlags::POSITION_VALID)
                && flags.contains(xr::SpaceLocationFlags::ORIENTATION_VALID)
            {
                return Ok(Some(location.pose));
            }
        }
        Ok(None)
    }

    pub fn scroll_axis(&self) -> Result<f32> {
        for action in [&self.right_thumbstick_action, &self.left_thumbstick_action] {
            let state = action.state(&self.session, xr::Path::NULL)?;
            if state.is_active && state.current_state.y.abs() > 0.15 {
                return Ok(state.current_state.y);
            }
        }
        Ok(0.0)
    }

    pub fn release_and_end_frame(&mut self, frame_data: &XrFrameData) -> Result<()> {
        if frame_data.views.len() < 2 {
            return Err(anyhow::anyhow!(
                "XR frame data has {} view(s), expected at least 2",
                frame_data.views.len()
            ));
        }

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
        if !is_valid_pose(&left_view.pose) || !is_valid_pose(&right_view.pose) {
            log::warn!("XR frame contains invalid projection pose; ending frame without layers.");
            self.frame_stream.end(
                frame_data.predicted_display_time,
                xr::EnvironmentBlendMode::OPAQUE,
                &[],
            )?;
            return Ok(());
        }

        let projection_views = [
            xr::CompositionLayerProjectionView::new()
                .pose(left_view.pose)
                .fov(left_view.fov)
                .sub_image(sub_image_left),
            xr::CompositionLayerProjectionView::new()
                .pose(right_view.pose)
                .fov(right_view.fov)
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
        (right + left) / w, (up + down) / h,    -far / (far - near),            -1.0,
        0.0,                0.0,                -(far * near) / (far - near),    0.0,
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

fn is_valid_pose(pose: &xr::Posef) -> bool {
    let q = &pose.orientation;
    let p = &pose.position;
    let finite = q.x.is_finite()
        && q.y.is_finite()
        && q.z.is_finite()
        && q.w.is_finite()
        && p.x.is_finite()
        && p.y.is_finite()
        && p.z.is_finite();
    if !finite {
        return false;
    }
    let norm_sq = q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w;
    norm_sq > 1e-6
}
