use super::*;

#[test]
fn exact_decimal_units_accept_full_u32_range_and_release_blanks() {
    for value in [1, 125, 1000, 900125, 123456, u32::MAX] {
        assert_eq!(parse_units(&format_units(value)).unwrap(), Some(value));
    }
    assert_eq!(parse_units("900.125").unwrap(), Some(900125));
    assert_eq!(parse_units("123.456").unwrap(), Some(123456));
    assert_eq!(parse_units("  ").unwrap(), None);
    for invalid in [
        "0",
        "-1",
        "1e3",
        "1.0001",
        "4294967.296",
        "1.",
        ".1",
        "NaN",
        "1,5",
    ] {
        assert!(parse_units(invalid).is_err(), "{invalid}");
    }
}

#[test]
fn merge_preserves_voltage_settings_when_protections_are_disabled() {
    let original = NvidiaRailControls {
        nvvdd: NvidiaRailSettings {
            min_voltage_uv: Some(900125),
            overvoltage_uv: Some(1000000),
            ..Default::default()
        },
        msvdd: NvidiaRailSettings {
            ocp_limit_ma: Some(123456),
            ..Default::default()
        },
    };
    let mut config = Some(original.clone());
    RailEdits::default().apply(&mut config).unwrap();
    assert_eq!(config, Some(original.clone()));
    let mut edits = RailEdits::default();
    edits.fields[0] = Some(String::new());
    edits.fields[1] = Some("950.125".to_owned());
    edits.apply(&mut config).unwrap();
    let merged = config.as_ref().unwrap();
    assert_eq!(merged.nvvdd.min_voltage_uv, None);
    assert_eq!(merged.nvvdd.max_voltage_uv, Some(950125));
    assert_eq!(merged.nvvdd.overvoltage_uv, original.nvvdd.overvoltage_uv);
    assert_eq!(merged.msvdd, original.msvdd);
    edits.enabled = Some(false);
    edits.apply(&mut config).unwrap();
    let voltage = config.as_ref().unwrap();
    assert_eq!(voltage.nvvdd.max_voltage_uv, Some(950125));
    assert_eq!(voltage.nvvdd.overvoltage_uv, None);
    assert_eq!(voltage.msvdd.ocp_limit_ma, None);
    RailEdits {
        restore_all: true,
        ..Default::default()
    }
    .apply(&mut config)
    .unwrap();
    assert_eq!(config, None);
    RailEdits {
        enabled: Some(true),
        ..Default::default()
    }
    .apply(&mut config)
    .unwrap();
    assert_eq!(config, Some(NvidiaRailControls::default()));
}

#[test]
fn invalid_edit_is_atomic_and_equal_minimum_maximum_remains_a_policy_request() {
    let mut config = Some(NvidiaRailControls::default());
    let mut edits = RailEdits::default();
    edits.fields[0] = Some("900.125".to_owned());
    edits.fields[1] = Some("invalid".to_owned());
    assert!(edits.validate().is_err());
    assert_eq!(edits.fields[0].as_deref(), Some("900.125"));
    assert_eq!(edits.fields[1].as_deref(), Some("invalid"));
    assert!(edits.apply(&mut config).is_err());
    assert_eq!(config, Some(NvidiaRailControls::default()));
    edits.fields[1] = Some("900.125".to_owned());
    assert!(edits.validate().is_ok());
    edits.apply(&mut config).unwrap();
    let rail = config.unwrap().nvvdd;
    assert_eq!(rail.min_voltage_uv, rail.max_voltage_uv);
    edits.fields[1] = Some("invalid".to_owned());
    edits.enabled = Some(false);
    assert!(edits.validate().is_err());
    edits.fields[1] = Some("900.125".to_owned());
    edits.fields[2] = Some("invalid".to_owned());
    assert!(edits.validate().is_ok());
    edits.restore_all = true;
    edits.fields[1] = Some("invalid".to_owned());
    assert!(edits.validate().is_ok());
}

#[test]
fn blank_voltage_releases_last_owned_field_and_noop_empty_config_is_preserved() {
    let mut config = Some(NvidiaRailControls::default());
    RailEdits::default().apply(&mut config).unwrap();
    assert!(config.is_some());
    config.as_mut().unwrap().nvvdd.min_voltage_uv = Some(900000);
    let mut edits = RailEdits::default();
    edits.fields[0] = Some(String::new());
    edits.apply(&mut config).unwrap();
    assert!(config.is_none());
}

fn synthetic_alt_vop() -> NvidiaAltVopPolicy {
    NvidiaAltVopPolicy {
        selector_present: true,
        selector: Some(264),
        evaluated_uv: Some(1135000),
        writable: false,
        identity: lact_schema::NvidiaAltVopIdentity {
            source: "runtime_rail_info".to_owned(),
            policy: "ALT_VOP".to_owned(),
            rail_index: 0,
            rail_enum: 1,
            object_type: 3,
        },
    }
}

#[test]
fn alt_vop_reads_as_text_only_and_legacy_voltage_as_the_rail_reference() {
    let mut alt_vop = synthetic_alt_vop();
    assert_eq!(
        alt_vop_reading(&alt_vop),
        fl!(
            I18N,
            "nvidia-rails-alt-vop",
            value = "1135",
            selector = "264"
        )
    );
    alt_vop.evaluated_uv = None;
    assert_eq!(
        alt_vop_reading(&alt_vop),
        fl!(I18N, "nvidia-rails-alt-vop-unknown", selector = "264")
    );
    alt_vop.selector_present = false;
    alt_vop.selector = None;
    assert_eq!(
        alt_vop_reading(&alt_vop),
        fl!(I18N, "nvidia-rails-alt-vop-absent")
    );
    assert_eq!(
        telemetry_reading("default", 900125),
        fl!(I18N, "nvidia-rails-reference", value = "900.125")
    );
}

#[test]
fn pending_rail_warning_does_not_replace_current_readings_or_requested_edits() {
    let info: NvidiaRuntimeInfo = serde_json::from_value(serde_json::json!({
        "protocol_version":1,"available":true,"device":{"uuid":"GPU-test","bdf":"0000:09:00.0"},
        "managed":false,"capabilities":{"rail_probe":1},"can_stage":false,"rails_pending":true,
        "deferred_fields":[{"rail":"MSVDD","field":"max_voltage_uv","operation":"set","reason":"R625 no rail SET"}],
        "rails":[{"rail":"NVVDD","max_voltage":{"evaluated_uv":1040000,"offset_uv":0,"writable":true}}]
    })).unwrap();
    let configured = NvidiaRailControls {
        nvvdd: NvidiaRailSettings {
            max_voltage_uv: Some(1125000),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut model = NvidiaRailsFrame {
        info: Some(info),
        is_nvidia: true,
        configured: Some(configured),
        edits: RailEdits::default(),
        rows: vec![],
        rail_sections: vec![],
        protection_sections: vec![],
        voltage_readbacks: vec![],
        loading: Rc::new(Cell::new(false)),
    };
    model.edits.fields[1] = Some("1150".into());
    assert!(model.pending_message().contains("R625 no rail SET"));
    assert!(model.readback(1).contains("1040"));
    assert_eq!(model.edits.fields[1].as_deref(), Some("1150"));
    model.info.as_mut().unwrap().available = false;
    model.info.as_mut().unwrap().rails[0]
        .max_voltage
        .as_mut()
        .unwrap()
        .writable = false;
    assert!(model.field_supported(1));
    assert!(model.readback(1).contains("1040"));
    assert!(!model.field_writable(1));
    assert_eq!(model.edits.fields[1].as_deref(), Some("1150"));
    model.info.as_mut().unwrap().can_stage = Some(true);
    assert!(!model.pending_message().is_empty());
    model.configured = None;
    assert!(model.can_restore());
    model.info.as_mut().unwrap().recovery_required = true;
    assert!(model.pending_message().is_empty());
    assert!(!model.field_writable(1));
}

#[cfg(feature = "gtk-tests")]
#[test]
#[ignore = "requires a GTK display; run this exact test explicitly"]
fn rail_panel_load_is_not_an_edit_and_unsupported_fields_are_preserved() {
    use crate::app::utils::ext::RelmDefaultLauchable;
    use relm4::ComponentController;
    adw::init().unwrap();
    let context = gtk::glib::MainContext::default();
    let _guard = context.acquire().unwrap();
    let drain = || {
        while context.pending() {
            context.iteration(false);
        }
    };
    let frame = NvidiaRailsFrame::detach_default();
    let configured = NvidiaRailControls {
        msvdd: NvidiaRailSettings {
            ocp_limit_ma: Some(123456),
            ..Default::default()
        },
        ..Default::default()
    };
    frame.emit(NvidiaRailsFrameMsg::Info {
        is_nvidia: true,
        info: None,
    });
    frame.emit(NvidiaRailsFrameMsg::Config(Some(configured.clone())));
    drain();
    assert!(frame.widgets().enabled.is_active());
    assert_eq!(frame.model().rows[7].entry.text(), "123.456");
    assert!(frame.model().protection_sections[1].get_visible());
    assert!(!frame.model().rows[7].entry.is_sensitive());
    let mut current = lact_schema::config::GpuConfig {
        nvidia_rail_controls: Some(configured.clone()),
        ..Default::default()
    };
    frame.model().apply_config(&mut current).unwrap();
    assert_eq!(current.nvidia_rail_controls, Some(configured));
    frame.emit(NvidiaRailsFrameMsg::Info {
        is_nvidia: true,
        info: Some(Box::new(NvidiaRuntimeInfo {
            protocol_version: 1,
            recovery_required: false,
            can_stage: None,
            deferred_fields: vec![],
            ownership_unavailable: false,
            rails_pending: false,
            capabilities: Default::default(),
            transaction_id: None,
            transaction_status: None,
            request_id: None,
            telemetry: Some(lact_schema::NvidiaRuntimeTelemetry {
                adc_samples: vec![lact_schema::NvidiaAdcSample {
                    sensor_id: 9,
                    rail: None,
                    voltage_uv: 875125,
                }],
                unavailable: vec![],
            }),
            available: true,
            reason: None,
            device: lact_schema::NvidiaRuntimeDevice {
                uuid: "GPU-synthetic".to_owned(),
                bdf: "0000:09:00.0".to_owned(),
            },
            managed: true,
            rails: vec![NvidiaRuntimeRail {
                rail: NvidiaRail::Nvvdd,
                min_voltage: Some(lact_schema::NvidiaVoltagePolicy {
                    evaluated_uv: 900125,
                    offset_uv: -125,
                    writable: true,
                    requested_uv: None,
                }),
                max_voltage: Some(lact_schema::NvidiaVoltagePolicy {
                    evaluated_uv: 1050000,
                    offset_uv: 0,
                    writable: true,
                    requested_uv: None,
                }),
                overvoltage: None,
                ocp_limit: None,
                effective_max_uv: Some(1050000),
                reported_voltage_uv: Some(875125),
                telemetry: Some(lact_schema::NvidiaRailTelemetry {
                    target_uv: Some(875125),
                    default_uv: Some(900125),
                    current_ma: Some(123456),
                    arbitrated_ocp_ma: Some(180000),
                    ..Default::default()
                }),
                alt_vop: Some(synthetic_alt_vop()),
            }],
        })),
    });
    drain();
    assert!(frame.model().rows[0].entry.is_sensitive());
    assert!(frame.model().rows[0].entry.text().is_empty());
    assert!(frame.model().rows[0].readback.text().contains("900.125"));
    assert!(
        frame.model().voltage_readbacks[0]
            .text()
            .contains("123.456")
    );
    assert!(frame.model().voltage_readbacks[0].text().contains("180"));
    // ALT/VOP is a separate read-only line; the REL entry stays its own control.
    assert!(
        frame.model().voltage_readbacks[0]
            .text()
            .contains(&alt_vop_reading(&synthetic_alt_vop()))
    );
    assert!(frame.widgets().telemetry_details.text().contains("875.125"));
    assert!(!frame.widgets().telemetry_details.text().contains("NVVDD"));
    frame.model().rows[0].entry.set_text("925.1250");
    drain();
    assert!(frame.model().validate().is_err());
    assert_eq!(frame.model().rows[0].entry.text(), "925.1250");
    assert_eq!(
        current
            .nvidia_rail_controls
            .as_ref()
            .unwrap()
            .msvdd
            .ocp_limit_ma,
        Some(123456)
    );
    frame.model().rows[0].entry.set_text("925.125");
    drain();
    let mut deferred_info = frame.model().info.clone().unwrap();
    deferred_info.capabilities.rail_probe = 1;
    deferred_info.can_stage = Some(false);
    deferred_info.rails_pending = true;
    deferred_info
        .deferred_fields
        .push(lact_schema::NvidiaDeferredRailField {
            rail: NvidiaRail::Msvdd,
            field: lact_schema::NvidiaRailField::OcpLimitMa,
            operation: lact_schema::NvidiaRailOperation::Set,
            reason: "synthetic R625 unsupported rail".into(),
        });
    frame.emit(NvidiaRailsFrameMsg::Info {
        is_nvidia: true,
        info: Some(Box::new(deferred_info)),
    });
    drain();
    assert_eq!(frame.model().rows[0].entry.text(), "925.125");
    assert!(frame.widgets().pending_warning.get_visible());
    assert!(
        frame
            .widgets()
            .pending_warning
            .text()
            .contains("synthetic R625 unsupported rail")
    );
    assert!(frame.model().rows[0].readback.text().contains("900.125"));
    frame.model().apply_config(&mut current).unwrap();
    assert_eq!(
        current
            .nvidia_rail_controls
            .as_ref()
            .unwrap()
            .nvvdd
            .min_voltage_uv,
        Some(925125)
    );
    assert_eq!(
        current
            .nvidia_rail_controls
            .as_ref()
            .unwrap()
            .msvdd
            .ocp_limit_ma,
        Some(123456)
    );
    frame.widgets().enabled.set_active(false);
    drain();
    assert!(frame.model().rows[0].entry.is_sensitive());
    frame.model().apply_config(&mut current).unwrap();
    let settings = current.nvidia_rail_controls.as_ref().unwrap();
    assert_eq!(settings.nvvdd.min_voltage_uv, Some(925125));
    assert_eq!(settings.msvdd.ocp_limit_ma, None);
    frame.model().rows[0].entry.set_text("");
    drain();
    assert!(frame.model().validate().is_ok());
    frame.model().apply_config(&mut current).unwrap();
    assert!(current.nvidia_rail_controls.is_none());
    frame.emit(NvidiaRailsFrameMsg::Config(None));
    drain();
    assert!(frame.model().rail_sections[0].get_visible());
    assert!(!frame.model().rail_sections[1].get_visible());
    // A pending helper transaction can be restored without reinterpreting protection opt-in.
    frame.emit(NvidiaRailsFrameMsg::Info {
        is_nvidia: true,
        info: Some(Box::new(NvidiaRuntimeInfo {
            protocol_version: 1,
            recovery_required: true,
            capabilities: Default::default(),
            transaction_id: None,
            transaction_status: None,
            request_id: None,
            telemetry: None,
            can_stage: None,
            deferred_fields: vec![],
            ownership_unavailable: false,
            rails_pending: false,
            available: false,
            reason: Some("Synthetic recovery required".to_owned()),
            device: lact_schema::NvidiaRuntimeDevice {
                uuid: "GPU-synthetic".to_owned(),
                bdf: "0000:09:00.0".to_owned(),
            },
            managed: false,
            rails: Vec::new(),
        })),
    });
    frame.emit(NvidiaRailsFrameMsg::Config(None));
    drain();
    assert!(!frame.widgets().enabled.is_active());
    assert!(!frame.widgets().enabled.is_sensitive());
    assert!(frame.widgets().restore.is_sensitive());
    assert!(!frame.model().rows[0].entry.is_sensitive());
    frame.widgets().restore.emit_clicked();
    drain();
    frame.model().apply_config(&mut current).unwrap();
    assert!(current.nvidia_rail_controls.is_none());
    frame.emit(NvidiaRailsFrameMsg::Enabled(true));
    drain();
    assert!(!frame.widgets().enabled.is_active());
    frame.emit(NvidiaRailsFrameMsg::Info {
        is_nvidia: false,
        info: None,
    });
    drain();
    assert!(!frame.widget().is_visible());
}
