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
    // The upstream component publishes PerformanceLevelChanged when loading stats.
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

    // The factory recreates the power limit row; it must stay above the toggle.
    let list = frame.model().rows.widget().clone();
    let toggle = (0..)
        .map_while(|index| list.row_at_index(index))
        .position(|row| row == frame.widgets().experimental_controls)
        .unwrap();
    let cap_row = (0..)
        .map_while(|index| list.row_at_index(index))
        .find(|row| row.has_css_class("adjustment-row"))
        .expect("the power cap SettingRow must be present");
    assert!(cap_row.is_visible());
    assert!(cap_row.index() < toggle as i32);

    // A user opt-in extends the range; opt-out clamps the pending edit and
    // exposes it to AppModel so an old sub-minimum config is not resubmitted.
    frame.widgets().ioctl_toggle.set_active(true);
    drain();
    assert_eq!(cap_min(), 30.0);
    frame.model().rows.send(
        &PowerSetting::PowerCap,
        SettingRowMsg::SetValue(SettingRowValue::Numeric(150.0)),
    );
    drain();
    let pending = frame.model().serialize();
    assert_eq!(pending.power_cap, Some(150.0));
    assert_eq!(
        pending.nvidia_power_cap_mode,
        Some(NvidiaPowerCapMode::Ioctl)
    );
    frame.widgets().ioctl_toggle.set_active(false);
    drain();
    assert_eq!(cap_min(), 250.0);
    assert_eq!(frame.model().serialize().power_cap, Some(250.0));

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
    frame.emit(PowerFrameMsg::Reset);
    drain();
    // SettingRow omits the default so applying Reset clears the stored cap.
    assert_eq!(frame.model().serialize().power_cap, None);
    assert_eq!(
        frame.model().nvidia_power_cap_mode(),
        Some(NvidiaPowerCapMode::Ioctl)
    );

    // Other drivers never see the toggle or report a mode.
    frame.emit(PowerFrameMsg::NvidiaMode(None));
    frame.emit(stats(native_stats));
    drain();
    assert!(!frame.widgets().experimental_controls.is_visible());
    assert_eq!(frame.model().nvidia_power_cap_mode(), None);
    assert_eq!(cap_min(), 250.0);
}
