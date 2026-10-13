use crate::{
    APP_BROKER, I18N,
    app::{components::page_section::PageSection, msg::AppMsg},
};
use adw::prelude::*;
use i18n_embed_fl::fl;
use lact_schema::{
    NvidiaAltVopPolicy, NvidiaRail, NvidiaRailControls, NvidiaRailSettings, NvidiaRuntimeInfo,
    NvidiaRuntimeRail,
};
use relm4::{ComponentParts, ComponentSender};
use std::{cell::Cell, rc::Rc};

pub struct NvidiaRailsFrame {
    info: Option<NvidiaRuntimeInfo>,
    is_nvidia: bool,
    configured: Option<NvidiaRailControls>,
    edits: RailEdits,
    rows: Vec<RailRow>,
    rail_sections: Vec<gtk::Box>,
    protection_sections: Vec<gtk::Box>,
    voltage_readbacks: Vec<gtk::Label>,
    loading: Rc<Cell<bool>>,
}

struct RailRow {
    container: gtk::Box,
    entry: gtk::Entry,
    readback: gtk::Label,
}

#[derive(Debug)]
pub enum NvidiaRailsFrameMsg {
    Info {
        is_nvidia: bool,
        info: Option<Box<NvidiaRuntimeInfo>>,
    },
    Config(Option<NvidiaRailControls>),
    Enabled(bool),
    RestoreAll,
    Edit(usize, String),
}

/// Only edited fields are merged into the freshly fetched configuration.
#[derive(Default)]
struct RailEdits {
    enabled: Option<bool>,
    restore_all: bool,
    fields: [Option<String>; 8],
}

#[relm4::component(pub)]
impl relm4::Component for NvidiaRailsFrame {
    type Init = ();
    type Input = NvidiaRailsFrameMsg;
    type Output = ();
    type CommandOutput = ();

    view! {
        #[root]
        gtk::Box {
            set_orientation: gtk::Orientation::Vertical,
            set_spacing: 20,
            #[watch]
            set_visible: model.is_nvidia,
            PageSection {
                set_name: fl!(I18N, "nvidia-rails-section"),
                append_child = &gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_spacing: 10,
                    #[name = "status"]
                    gtk::Label {
                        set_wrap: true,
                        set_xalign: 0.0,
                        set_max_width_chars: 60,
                    },
                    #[name = "pending_warning"]
                    gtk::Label {
                        set_wrap: true,
                        set_xalign: 0.0,
                        set_max_width_chars: 60,
                        add_css_class: relm4::css::WARNING,
                    },
                    gtk::Label {
                        set_label: &fl!(I18N, "nvidia-rails-policy-note"),
                        set_wrap: true,
                        set_xalign: 0.0,
                        set_max_width_chars: 60,
                    },
                    gtk::Box {
                        set_spacing: 8,
                        gtk::Button {
                            set_label: &fl!(I18N, "nvidia-rails-refresh"),
                            connect_clicked => move |_| {
                                APP_BROKER.send(AppMsg::RefreshNvidiaRuntime);
                            },
                        },
                        #[name = "restore"]
                        gtk::Button {
                            set_label: &fl!(I18N, "nvidia-rails-restore-all"),
                            #[watch]
                            set_sensitive: model.can_restore(),
                            connect_clicked => NvidiaRailsFrameMsg::RestoreAll,
                        },
                    },
                    #[local_ref]
                    rails_box -> gtk::Box {},
                    gtk::Expander {
                        set_label: Some(&fl!(I18N, "nvidia-rails-sensors")),
                        #[watch]
                        set_visible: !model.telemetry_details().is_empty(),
                        #[name = "telemetry_details"]
                        gtk::Label {
                            set_wrap: true,
                            set_xalign: 0.0,
                            set_max_width_chars: 60,
                        },
                    },
                },
            },
            PageSection {
                set_name: fl!(I18N, "nvidia-rails-protections-section"),
                append_child = &gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_spacing: 10,
                    #[name = "enabled"]
                    gtk::CheckButton {
                        set_label: Some(&fl!(I18N, "nvidia-rails-enable")),
                        #[watch]
                        #[block_signal(enabled_handler)]
                        set_active: model.enabled(),
                        #[watch]
                        set_sensitive: model.enabled() || model.protections_available(),
                        connect_toggled[sender] => move |button| {
                            sender.input(NvidiaRailsFrameMsg::Enabled(button.is_active()));
                        } @ enabled_handler,
                    },
                    gtk::Label {
                        set_label: &fl!(I18N, "nvidia-rails-protections-note"),
                        set_wrap: true,
                        set_xalign: 0.0,
                        set_max_width_chars: 60,
                    },
                    #[local_ref]
                    protections_box -> gtk::Box {},
                },
            },
        }
    }

    fn init(_: (), root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let loading = Rc::new(Cell::new(false));
        let rails_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let protections_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let mut rows = Vec::new();
        let mut rail_sections = Vec::new();
        let mut protection_sections = Vec::new();
        let mut voltage_readbacks = Vec::new();
        for (rail_index, name) in ["NVVDD", "MSVDD"].into_iter().enumerate() {
            let section = gtk::Box::new(gtk::Orientation::Vertical, 8);
            let title = gtk::Label::new(Some(name));
            title.set_xalign(0.0);
            title.add_css_class("heading");
            section.append(&title);
            let voltage_readback = gtk::Label::new(None);
            voltage_readback.set_wrap(true);
            voltage_readback.set_xalign(0.0);
            voltage_readback.add_css_class("dim-label");
            section.append(&voltage_readback);
            voltage_readbacks.push(voltage_readback);
            let protection_section = gtk::Box::new(gtk::Orientation::Vertical, 8);
            let protection_title = gtk::Label::new(Some(name));
            protection_title.set_xalign(0.0);
            protection_title.add_css_class("heading");
            protection_section.append(&protection_title);
            for field in 0..4 {
                let title = match field {
                    0 => fl!(I18N, "nvidia-rails-minimum"),
                    1 => fl!(I18N, "nvidia-rails-maximum"),
                    2 => fl!(I18N, "nvidia-rails-overvoltage"),
                    _ => fl!(I18N, "nvidia-rails-ocp"),
                };
                let container = gtk::Box::new(gtk::Orientation::Vertical, 3);
                let input = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let label = gtk::Label::new(Some(&title));
                label.set_xalign(0.0);
                label.set_hexpand(true);
                let entry = gtk::Entry::new();
                entry.set_width_chars(10);
                entry.set_input_purpose(gtk::InputPurpose::Number);
                entry.set_placeholder_text(Some(&fl!(I18N, "nvidia-rails-original")));
                let unit = gtk::Label::new(Some(if field == 3 { "A" } else { "mV" }));
                input.append(&label);
                input.append(&entry);
                input.append(&unit);
                container.append(&input);
                let readback = gtk::Label::new(None);
                readback.set_wrap(true);
                readback.set_xalign(0.0);
                readback.add_css_class("dim-label");
                container.append(&readback);
                let change_sender = sender.clone();
                let loading = loading.clone();
                let index = rail_index * 4 + field;
                entry.connect_changed(move |entry| {
                    if !loading.get() {
                        change_sender
                            .input(NvidiaRailsFrameMsg::Edit(index, entry.text().to_string()));
                    }
                });
                if field < 2 {
                    section.append(&container);
                } else {
                    protection_section.append(&container);
                }
                rows.push(RailRow {
                    container,
                    entry,
                    readback,
                });
            }
            rails_box.append(&section);
            protections_box.append(&protection_section);
            rail_sections.push(section);
            protection_sections.push(protection_section);
        }
        let model = Self {
            info: None,
            is_nvidia: false,
            configured: None,
            edits: RailEdits::default(),
            rows,
            rail_sections,
            protection_sections,
            voltage_readbacks,
            loading,
        };
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        msg: Self::Input,
        sender: ComponentSender<Self>,
        _: &Self::Root,
    ) {
        match msg {
            NvidiaRailsFrameMsg::Info { is_nvidia, info } => {
                self.is_nvidia = is_nvidia;
                self.info = info.map(|info| *info);
            }
            NvidiaRailsFrameMsg::Config(config) => {
                self.configured = config;
                self.edits = RailEdits::default();
                self.loading.set(true);
                for (index, row) in self.rows.iter().enumerate() {
                    let value = self
                        .configured
                        .as_ref()
                        .and_then(|config| field_value(config, index));
                    row.entry
                        .set_text(&value.map(format_units).unwrap_or_default());
                }
                self.loading.set(false);
            }
            NvidiaRailsFrameMsg::Enabled(enabled) => {
                if !enabled || self.protections_available() {
                    self.edits.restore_all = false;
                    self.edits.enabled = Some(enabled);
                    APP_BROKER.send(AppMsg::SettingsChanged);
                }
            }
            NvidiaRailsFrameMsg::RestoreAll => {
                self.edits.restore_all = true;
                self.edits.enabled = Some(false);
                self.edits.fields = std::array::from_fn(|_| Some(String::new()));
                self.loading.set(true);
                for row in &self.rows {
                    row.entry.set_text("");
                }
                self.loading.set(false);
                APP_BROKER.send(AppMsg::SettingsChanged);
            }
            NvidiaRailsFrameMsg::Edit(index, value) => {
                if self.editable(index) {
                    self.edits.restore_all = false;
                    self.edits.fields[index] = Some(value);
                    APP_BROKER.send(AppMsg::SettingsChanged);
                }
            }
        }
        widgets.status.set_label(
            &self
                .info
                .as_ref()
                .filter(|info| info.available)
                .map(|_| fl!(I18N, "nvidia-rails-available"))
                .unwrap_or_else(|| {
                    self.info
                        .as_ref()
                        .and_then(|info| info.reason.clone())
                        .unwrap_or_else(|| fl!(I18N, "nvidia-rails-unavailable"))
                }),
        );
        let pending = self.pending_message();
        widgets.pending_warning.set_label(&pending);
        widgets.pending_warning.set_visible(!pending.is_empty());
        for (index, row) in self.rows.iter().enumerate() {
            row.entry.set_sensitive(self.editable(index));
            let available = self.field_supported(index);
            let configured = self
                .configured
                .as_ref()
                .and_then(|config| field_value(config, index))
                .is_some();
            row.container
                .set_visible(available || configured || self.edits.fields[index].is_some());
            row.readback.set_label(&self.readback(index));
            let invalid = self.edits.fields[index]
                .as_deref()
                .is_some_and(|text| parse_units(text).is_err());
            if invalid {
                row.entry.add_css_class("error");
            } else {
                row.entry.remove_css_class("error");
            }
        }
        for (index, section) in self.rail_sections.iter().enumerate() {
            let rail = self.rail(index * 4);
            let readback = rail
                .map(|rail| {
                    let mut readings = Vec::new();
                    // Older providers send the rail reference only under this name.
                    if let Some(value) = rail.reported_voltage_uv
                        && rail
                            .telemetry
                            .as_ref()
                            .and_then(|telemetry| telemetry.default_uv)
                            .is_none()
                    {
                        readings.push(telemetry_reading("default", value));
                    }
                    if let Some(value) = rail.effective_max_uv {
                        readings.push(fl!(
                            I18N,
                            "nvidia-rails-effective",
                            value = format_units(value)
                        ));
                    }
                    if let Some(alt_vop) = &rail.alt_vop {
                        readings.push(alt_vop_reading(alt_vop));
                    }
                    if let Some(telemetry) = &rail.telemetry {
                        for (name, value) in [
                            ("target", telemetry.target_uv),
                            ("default", telemetry.default_uv),
                            ("sensed", telemetry.sensed_uv),
                            ("adc", telemetry.adc_uv),
                            ("current", telemetry.current_ma),
                            ("arbitrated", telemetry.arbitrated_ocp_ma),
                        ] {
                            if let Some(value) = value {
                                readings.push(telemetry_reading(name, value));
                            }
                        }
                    }
                    readings.join("\n")
                })
                .unwrap_or_default();
            self.voltage_readbacks[index].set_label(&readback);
            self.voltage_readbacks[index].set_visible(!readback.is_empty());
            section.set_visible(
                !readback.is_empty()
                    || self.rows[index * 4..index * 4 + 2]
                        .iter()
                        .any(|row| row.container.get_visible()),
            );
            self.protection_sections[index].set_visible(
                self.rows[index * 4 + 2..index * 4 + 4]
                    .iter()
                    .any(|row| row.container.get_visible()),
            );
        }
        widgets
            .telemetry_details
            .set_label(&self.telemetry_details());
        self.update_view(widgets, sender);
    }
}

impl NvidiaRailsFrame {
    fn pending_message(&self) -> String {
        let info = self.info.as_ref();
        if info.is_some_and(|info| info.recovery_required) {
            return String::new();
        }
        let saved = self
            .configured
            .as_ref()
            .is_some_and(|settings| (0..8).any(|index| field_value(settings, index).is_some()));
        let pending = info.is_some_and(|info| {
            info.rails_pending
                || !info.deferred_fields.is_empty()
                || (saved && info.can_stage == Some(false))
        }) || (saved && info.is_none());
        if !pending {
            return String::new();
        }
        let reason = info
            .map(|info| {
                if info.deferred_fields.is_empty() {
                    info.reason
                        .clone()
                        .unwrap_or_else(|| fl!(I18N, "nvidia-rails-unavailable"))
                } else {
                    info.deferred_fields
                        .iter()
                        .map(|field| {
                            let rail = match field.rail {
                                NvidiaRail::Nvvdd => "NVVDD",
                                NvidiaRail::Msvdd => "MSVDD",
                            };
                            let policy = match field.field {
                                lact_schema::NvidiaRailField::MinVoltageUv => "VMIN",
                                lact_schema::NvidiaRailField::MaxVoltageUv => "REL",
                                lact_schema::NvidiaRailField::OvervoltageUv => "OV",
                                lact_schema::NvidiaRailField::OcpLimitMa => "OCP",
                            };
                            format!("{rail} {policy}: {}", field.reason)
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                }
            })
            .unwrap_or_else(|| fl!(I18N, "nvidia-rails-unavailable"));
        fl!(I18N, "nvidia-rails-pending", reason = reason)
    }

    fn telemetry_details(&self) -> String {
        let Some(telemetry) = self.info.as_ref().and_then(|info| info.telemetry.as_ref()) else {
            return String::new();
        };
        let mut readings = Vec::new();
        for sample in &telemetry.adc_samples {
            let rail = match sample.rail {
                Some(NvidiaRail::Nvvdd) => "NVVDD".to_owned(),
                Some(NvidiaRail::Msvdd) => "MSVDD".to_owned(),
                None => fl!(I18N, "nvidia-rails-unknown-sensor-rail"),
            };
            readings.push(fl!(
                I18N,
                "nvidia-rails-adc-sample",
                sensor = sample.sensor_id,
                rail = rail,
                value = format_units(sample.voltage_uv)
            ));
        }
        for unavailable in &telemetry.unavailable {
            readings.push(fl!(
                I18N,
                "nvidia-rails-telemetry-unavailable",
                feature = unavailable.feature.clone(),
                reason = unavailable.reason.clone()
            ));
        }
        readings.join("\n")
    }

    fn enabled(&self) -> bool {
        self.edits.enabled.unwrap_or_else(|| {
            self.configured.as_ref().is_some_and(|config| {
                [2, 3, 6, 7]
                    .into_iter()
                    .any(|index| field_value(config, index).is_some())
            })
        })
    }

    fn protections_available(&self) -> bool {
        [2, 3, 6, 7]
            .into_iter()
            .any(|index| self.field_writable(index))
    }

    fn editable(&self, index: usize) -> bool {
        self.field_writable(index) && (index % 4 < 2 || self.enabled())
    }

    fn can_restore(&self) -> bool {
        self.configured.is_some()
            || self
                .info
                .as_ref()
                .is_some_and(|info| info.managed || info.recovery_required || info.rails_pending)
            || self.edits.fields.iter().any(Option::is_some)
    }

    fn rail(&self, index: usize) -> Option<&NvidiaRuntimeRail> {
        let rail = if index < 4 {
            NvidiaRail::Nvvdd
        } else {
            NvidiaRail::Msvdd
        };
        self.info
            .as_ref()?
            .rails
            .iter()
            .find(|info| info.rail == rail)
    }

    fn field_supported(&self, index: usize) -> bool {
        self.rail(index).is_some_and(|rail| match index % 4 {
            0 => rail.min_voltage.is_some(),
            1 => rail.max_voltage.is_some(),
            2 => rail.overvoltage.is_some(),
            _ => rail.ocp_limit.is_some(),
        })
    }

    fn field_writable(&self, index: usize) -> bool {
        self.info.as_ref().is_some_and(|info| {
            info.available
                && !info.recovery_required
                && !matches!(
                    info.transaction_status.as_deref(),
                    Some("prepared" | "staged" | "rollback_required")
                )
        }) && self.rail(index).is_some_and(|rail| match index % 4 {
            0 => rail
                .min_voltage
                .as_ref()
                .is_some_and(|policy| policy.writable),
            1 => rail
                .max_voltage
                .as_ref()
                .is_some_and(|policy| policy.writable),
            2 => rail
                .overvoltage
                .as_ref()
                .is_some_and(|policy| policy.writable),
            _ => rail.ocp_limit.as_ref().is_some_and(|limit| limit.writable),
        })
    }

    fn readback(&self, index: usize) -> String {
        let Some(rail) = self.rail(index) else {
            return fl!(I18N, "nvidia-rails-unavailable");
        };
        if index % 4 == 3 {
            rail.ocp_limit
                .as_ref()
                .map(|limit| {
                    fl!(
                        I18N,
                        "nvidia-rails-current-readback",
                        current = format_units(limit.current_ma),
                        minimum = format_units(limit.minimum_ma),
                        maximum = format_units(limit.maximum_ma)
                    )
                })
                .unwrap_or_else(|| fl!(I18N, "nvidia-rails-unavailable"))
        } else {
            let policy = match index % 4 {
                0 => &rail.min_voltage,
                1 => &rail.max_voltage,
                _ => &rail.overvoltage,
            };
            policy
                .as_ref()
                .map(|policy| {
                    fl!(
                        I18N,
                        "nvidia-rails-voltage-readback",
                        evaluated = format_units(policy.evaluated_uv)
                    )
                })
                .unwrap_or_else(|| fl!(I18N, "nvidia-rails-unavailable"))
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        self.edits.validate()
    }

    pub fn apply_config(&self, config: &mut lact_schema::config::GpuConfig) -> anyhow::Result<()> {
        self.edits.apply(&mut config.nvidia_rail_controls)
    }
}

impl RailEdits {
    fn validate(&self) -> anyhow::Result<()> {
        if !self.restore_all {
            for (index, field) in self.fields.iter().enumerate() {
                if (index % 4 < 2 || self.enabled != Some(false))
                    && let Some(field) = field
                {
                    parse_units(field)?;
                }
            }
        }
        Ok(())
    }

    fn apply(&self, config: &mut Option<NvidiaRailControls>) -> anyhow::Result<()> {
        if self.restore_all {
            *config = None;
            return Ok(());
        }
        if self.enabled.is_none() && self.fields.iter().all(Option::is_none) {
            return Ok(());
        }
        let mut controls = config.clone().unwrap_or_default();
        for (index, edit) in self.fields.iter().enumerate() {
            if self.enabled == Some(false) && index % 4 >= 2 {
                *field_mut(&mut controls, index) = None;
            } else if let Some(edit) = edit {
                *field_mut(&mut controls, index) = parse_units(edit)?;
            }
        }
        *config = if controls == NvidiaRailControls::default() && self.enabled != Some(true) {
            None
        } else {
            Some(controls)
        };
        Ok(())
    }
}

fn telemetry_reading(name: &str, value: u32) -> String {
    let value = format_units(value);
    match name {
        "target" => fl!(I18N, "nvidia-rails-target", value = value),
        "default" => fl!(I18N, "nvidia-rails-reference", value = value),
        "sensed" => fl!(I18N, "nvidia-rails-sensed", value = value),
        "adc" => fl!(I18N, "nvidia-rails-adc", value = value),
        "current" => fl!(I18N, "nvidia-rails-current", value = value),
        _ => fl!(I18N, "nvidia-rails-arbitrated-ocp", value = value),
    }
}

/// A runtime reading only: ALT/VOP is neither REL nor an editable request.
fn alt_vop_reading(alt_vop: &NvidiaAltVopPolicy) -> String {
    if !alt_vop.selector_present {
        return fl!(I18N, "nvidia-rails-alt-vop-absent");
    }
    let selector = alt_vop
        .selector
        .map_or_else(|| "?".to_owned(), |selector| selector.to_string());
    match alt_vop.evaluated_uv {
        Some(value) => fl!(
            I18N,
            "nvidia-rails-alt-vop",
            value = format_units(value),
            selector = selector
        ),
        None => fl!(I18N, "nvidia-rails-alt-vop-unknown", selector = selector),
    }
}

fn rail_settings(config: &NvidiaRailControls, index: usize) -> &NvidiaRailSettings {
    if index < 4 {
        &config.nvvdd
    } else {
        &config.msvdd
    }
}

fn field_value(config: &NvidiaRailControls, index: usize) -> Option<u32> {
    let rail = rail_settings(config, index);
    match index % 4 {
        0 => rail.min_voltage_uv,
        1 => rail.max_voltage_uv,
        2 => rail.overvoltage_uv,
        _ => rail.ocp_limit_ma,
    }
}

fn field_mut(config: &mut NvidiaRailControls, index: usize) -> &mut Option<u32> {
    let rail = if index < 4 {
        &mut config.nvvdd
    } else {
        &mut config.msvdd
    };
    match index % 4 {
        0 => &mut rail.min_voltage_uv,
        1 => &mut rail.max_voltage_uv,
        2 => &mut rail.overvoltage_uv,
        _ => &mut rail.ocp_limit_ma,
    }
}

fn format_units(value: u32) -> String {
    let whole = value / 1000;
    let fraction = value % 1000;
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{fraction:03}")
            .trim_end_matches('0')
            .to_owned()
    }
}

/// Parse mV or A into exact integer uV or mA without floating point rounding.
fn parse_units(text: &str) -> anyhow::Result<Option<u32>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let invalid = || anyhow::anyhow!(fl!(I18N, "nvidia-rails-invalid-value"));
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty()
        || !whole.bytes().all(|c| c.is_ascii_digit())
        || fraction.len() > 3
        || !fraction.bytes().all(|c| c.is_ascii_digit())
        || (text.contains('.') && fraction.is_empty())
    {
        return Err(invalid());
    }
    let whole: u32 = whole.parse().map_err(|_| invalid())?;
    let fraction: u32 = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u32>().map_err(|_| invalid())? * 10u32.pow(3 - fraction.len() as u32)
    };
    let value = whole
        .checked_mul(1000)
        .and_then(|value| value.checked_add(fraction))
        .filter(|value| *value > 0)
        .ok_or_else(invalid)?;
    Ok(Some(value))
}

#[cfg(test)]
#[path = "nvidia_rails_frame_tests.rs"]
mod tests;
