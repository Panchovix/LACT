use super::*;
use crate::app::utils::ext::RelmDefaultLauchable;

#[test]
#[ignore = "requires a GTK display; run explicitly with --ignored"]
fn experimental_power_mode_is_explicit_and_restores_native_bounds() {
    adw::init().unwrap();
    let context = gtk::glib::MainContext::default();
    let _guard = context.acquire().unwrap();
    let drain = || {
        while context.pending() {
            context.iteration(false);
        }
    };
    // Stats also publishes PerformanceLevelChanged in the upstream component.
    let (output, _output_receiver) = relm4::channel::<OcPageMsg>();
    let frame = PowerFrame::launch_default().forward(&output, |msg| msg);
    let cap_min = || frame.model().cap_min(frame.model().power.cap_min);
    let stats = |power| {
        PowerFrameMsg::Stats(Arc::new(DeviceStats {
            power,
            ..Default::default()
        }))
    };
    let native_stats = PowerStats {
        cap_current: Some(300.0),
        cap_min: Some(250.0),
        cap_max: Some(325.0),
        cap_default: Some(300.0),
        ..Default::default()
    };
    frame.emit(PowerFrameMsg::NvidiaMode(Some(NvidiaPowerCapMode::Nvml)));
    frame.emit(stats(native_stats.clone()));
    drain();
    assert!(frame.widgets().experimental_controls.is_visible());
    assert!(!frame.widgets().ioctl_toggle.is_active());
    assert_eq!(cap_min(), 250.0);
    assert_eq!(frame.model().serialize().power_cap, None);

    // A stale NVML display must not overwrite a freshly fetched IOCTL config
    // while another panel applies a REL-only edit.
    let fresh = lact_schema::config::GpuConfig {
        nvidia_power_cap_mode: NvidiaPowerCapMode::Ioctl,
        power_cap: Some(230.0),
        fan_control_enabled: true,
        nvidia_rail_controls: Some(lact_schema::NvidiaRailControls {
            nvvdd: lact_schema::NvidiaRailSettings {
                max_voltage_uv: Some(1125000),
                overvoltage_uv: Some(1150000),
                ..Default::default()
            },
            msvdd: lact_schema::NvidiaRailSettings {
                min_voltage_uv: Some(800000),
                ..Default::default()
            },
        }),
        ..Default::default()
    };
    let mut merged = fresh.clone();
    merged
        .nvidia_rail_controls
        .as_mut()
        .unwrap()
        .nvvdd
        .max_voltage_uv = Some(1100000);
    let expected = merged.clone();
    let power = frame.model().serialize();
    assert!(!power.power_cap_changed);
    if power.power_cap_changed {
        merged.power_cap = power.power_cap;
    }
    if let Some(mode) = frame.model().nvidia_power_cap_mode() {
        merged.nvidia_power_cap_mode = mode;
    }
    assert_eq!(
        merged, expected,
        "untouched power controls changed a REL-only merge"
    );
    assert_eq!(frame.model().nvidia_power_cap_mode(), None);

    // The factory recreates the power limit row; it must stay above the toggle.
    let list = frame.model().rows.widget().clone();
    let toggle = (0..)
        .map_while(|index| list.row_at_index(index))
        .position(|row| row == frame.widgets().experimental_controls)
        .unwrap();
    assert!((0..toggle).any(|index| list.row_at_index(index as i32).unwrap().is_visible()));

    // An explicit reset must clear a stale stored cap even if the UI is already default.
    frame.emit(PowerFrameMsg::Reset);
    drain();
    assert!(frame.model().serialize().power_cap_changed);
    assert_eq!(frame.model().serialize().power_cap, None);
    frame.emit(stats(native_stats.clone()));
    drain();
    assert!(!frame.model().serialize().power_cap_changed);

    // A user opt-in extends the range; opt-out clamps the pending edit and
    // exposes it to AppModel so an old sub-minimum config is not resubmitted.
    frame.widgets().ioctl_toggle.set_active(true);
    drain();
    assert_eq!(cap_min(), 30.0);
    assert_eq!(
        frame.model().nvidia_power_cap_mode(),
        Some(NvidiaPowerCapMode::Ioctl)
    );
    frame.model().rows.send(
        &PowerSetting::PowerCap,
        SettingRowMsg::SetValue(SettingRowValue::Numeric(150.0)),
    );
    drain();
    let pending = frame.model().serialize();
    assert_eq!(pending.power_cap, Some(150.0));
    assert!(pending.power_cap_changed);
    assert_eq!(
        pending.nvidia_power_cap_mode,
        Some(NvidiaPowerCapMode::Ioctl)
    );
    frame.widgets().ioctl_toggle.set_active(false);
    drain();
    assert_eq!(cap_min(), 250.0);
    assert_eq!(frame.model().serialize().power_cap, Some(250.0));
    assert_eq!(frame.model().nvidia_power_cap_mode(), None);

    // Restore an opted-in profile and its lower cap together, without
    // treating programmatic checkbox updates as user mode changes.
    frame.emit(PowerFrameMsg::NvidiaMode(Some(NvidiaPowerCapMode::Ioctl)));
    frame.emit(stats(PowerStats {
        cap_current: Some(150.0),
        ..native_stats.clone()
    }));
    drain();
    assert!(frame.widgets().ioctl_toggle.is_active());
    assert_eq!(
        frame
            .model()
            .rows
            .get(&PowerSetting::PowerCap)
            .unwrap()
            .get_value(),
        SettingRowValue::Numeric(150.0)
    );
    assert_eq!(frame.model().serialize().power_cap, Some(150.0));
    assert!(!frame.model().serialize().power_cap_changed);
    frame.emit(PowerFrameMsg::Reset);
    drain();
    assert_eq!(frame.model().serialize().power_cap, None);
    assert!(frame.model().serialize().power_cap_changed);
    assert_eq!(frame.model().nvidia_power_cap_mode(), None);
    frame.widgets().ioctl_toggle.set_active(false);
    drain();
    assert_eq!(
        frame.model().nvidia_power_cap_mode(),
        Some(NvidiaPowerCapMode::Nvml)
    );
    frame.widgets().ioctl_toggle.set_active(true);
    drain();
    assert_eq!(frame.model().nvidia_power_cap_mode(), None);
    frame.emit(stats(native_stats.clone()));
    drain();
    assert!(frame.widgets().ioctl_toggle.is_active());
    assert_eq!(frame.model().nvidia_power_cap_mode(), None);

    // Other drivers never see the toggle or report a mode.
    frame.emit(PowerFrameMsg::NvidiaMode(None));
    frame.emit(stats(native_stats));
    drain();
    assert!(!frame.widgets().experimental_controls.is_visible());
    assert_eq!(frame.model().nvidia_power_cap_mode(), None);
    assert_eq!(cap_min(), 250.0);
}
