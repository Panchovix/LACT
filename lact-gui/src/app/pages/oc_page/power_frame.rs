use crate::{
    APP_BROKER, I18N,
    app::{
        components::{
            adjustment_card::AdjustmentCard,
            page_section::PageSection,
            setting_row::{
                AdjustmentControlMsg, AdjustmentInit, SelectInit, SelectOption, SettingControl,
                SettingRow, SettingRowInit, SettingRowMsg, SettingRowValue,
            },
        },
        msg::AppMsg,
        pages::oc_page::{
            OcPageMsg,
            power_profile_mode::{AmdPowerProfileMode, AmdPowerProfileModeMsg},
        },
    },
};
use adw::prelude::*;
use amdgpu_sysfs::gpu_handle::{PerformanceLevel, power_profile_mode::PowerProfileModesTable};
use i18n_embed_fl::fl;
use lact_schema::{DeviceStats, PowerStats, config::NvidiaPowerCapMode};
use nvml_wrapper::enums::device::PowerMizerMode;
use relm4::{
    ComponentController, ComponentParts, ComponentSender, Controller, WidgetTemplate,
    factory::FactoryHashMap,
};
use std::sync::Arc;

const PERFORMANCE_LEVELS: [PerformanceLevel; 8] = [
    PerformanceLevel::Auto,
    PerformanceLevel::High,
    PerformanceLevel::Low,
    PerformanceLevel::Manual,
    PerformanceLevel::ProfileStandard,
    PerformanceLevel::ProfileMinSclk,
    PerformanceLevel::ProfileMinMclk,
    PerformanceLevel::ProfilePeak,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PowerSetting {
    PowerCap,
    PerformanceLevel,
    PowerMizer,
}

pub struct PowerFrameSerialized {
    pub power_cap: Option<f64>,
    pub nvidia_power_cap_mode: Option<NvidiaPowerCapMode>,
    pub performance_level: Option<PerformanceLevel>,
    pub power_mizer_mode: Option<PowerMizerMode>,
    pub power_profile_mode_index: Option<u16>,
    pub custom_power_profile_mode_hueristics: Vec<Vec<Option<i32>>>,
}

pub struct PowerFrame {
    power: PowerStats,
    nvidia_mode: Option<NvidiaPowerCapMode>,
    rows: FactoryHashMap<PowerSetting, SettingRow<PowerSetting>>,
    supported_power_mizer_modes: Vec<PowerMizerMode>,
    power_profile_mode: Option<Controller<AmdPowerProfileMode>>,
}

#[derive(Debug)]
pub enum PowerFrameMsg {
    Stats(Arc<DeviceStats>),
    /// `None` unless the GPU uses the NVIDIA driver.
    NvidiaMode(Option<NvidiaPowerCapMode>),
    ToggleNvidiaIoctl(bool),
    SetPerformanceLevel(PerformanceLevel),
    PowerProfileModes(Option<PowerProfileModesTable>),
    SettingChanged(PowerSetting),
    Reset,
}

#[relm4::component(pub)]
impl relm4::Component for PowerFrame {
    type Init = ();
    type Input = PowerFrameMsg;
    type Output = OcPageMsg;
    type CommandOutput = ();

    view! {
        #[root]
        PageSection {
            set_name: fl!(I18N, "power-section"),
            set_hide_visible_container: true,
            #[watch]
            set_visible: !model.rows.is_empty() || model.power_profile_mode.is_some(),

            append_header = &gtk::Button {
                set_label: &fl!(I18N, "default-button"),
                connect_clicked => PowerFrameMsg::Reset,
                set_halign: gtk::Align::End,
                set_hexpand: true,
                #[watch]
                set_visible: !model.rows.is_empty(),
            },
            #[template]
            #[local]
            append_child = &card -> AdjustmentCard {
                #[template_child]
                content {
                    #[name = "experimental_controls"]
                    gtk::ListBoxRow {
                        set_activatable: false,
                        set_selectable: false,
                        #[watch]
                        set_visible: model.nvidia_mode_available(),

                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 6,

                            #[name = "ioctl_toggle"]
                            gtk::CheckButton {
                                set_label: Some(&fl!(I18N, "nvidia-power-cap-ioctl")),
                                #[watch]
                                #[block_signal(ioctl_toggled_handler)]
                                set_active: model.nvidia_mode == Some(NvidiaPowerCapMode::Ioctl),
                                connect_toggled[sender] => move |button| {
                                    sender.input(PowerFrameMsg::ToggleNvidiaIoctl(button.is_active()));
                                } @ ioctl_toggled_handler,
                            },
                            gtk::Label {
                                set_label: &fl!(I18N, "nvidia-power-cap-ioctl-warning"),
                                set_wrap: true,
                                set_max_width_chars: 55,
                                set_xalign: 0.0,
                                add_css_class: relm4::css::WARNING,
                            },
                        },
                    },
                },
            },
        },
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let card = AdjustmentCard::init(());
        let model = Self {
            power: PowerStats::default(),
            nvidia_mode: None,
            rows: FactoryHashMap::builder()
                .launch(card.content.clone())
                .forward(sender.input_sender(), |(setting, _)| {
                    PowerFrameMsg::SettingChanged(setting)
                }),
            supported_power_mizer_modes: Vec::new(),
            power_profile_mode: None,
        };
        let widgets = view_output!();

        let experimental_controls = widgets.experimental_controls.clone();
        model.rows.widget().set_sort_func(move |left, right| {
            // Keep the cap first, then the opt-in, performance settings, and AMD profile.
            let rank = |row: &gtk::ListBoxRow| {
                if row.has_css_class("adjustment-row") {
                    0
                } else if row == &experimental_controls {
                    1
                } else if row.has_css_class("power-profile-mode") {
                    3
                } else {
                    2
                }
            };
            rank(left).cmp(&rank(right)).into()
        });

        ComponentParts { model, widgets }
    }

    fn update_with_view(
        &mut self,
        widgets: &mut Self::Widgets,
        msg: Self::Input,
        sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match msg {
            PowerFrameMsg::Stats(stats) => {
                self.rows.clear();
                let power = &stats.power;
                if let Some(value) = power.cap_current
                    && let Some(lower) = power.cap_min
                    && let Some(upper) = power.cap_max
                {
                    self.rows.insert(
                        PowerSetting::PowerCap,
                        SettingRowInit {
                            title: fl!(I18N, "power-cap"),
                            control: AdjustmentInit {
                                unit: fl!(I18N, "watt"),
                                value,
                                default_value: power.cap_default,
                                lower: self.cap_min(Some(lower)),
                                upper,
                                ..Default::default()
                            }
                            .into(),
                            ..Default::default()
                        },
                    );
                }

                if let Some(level) = stats.performance_level {
                    self.rows.insert(
                        PowerSetting::PerformanceLevel,
                        SettingRowInit {
                            title: fl!(I18N, "performance-level"),
                            control: SelectInit {
                                default_value: PERFORMANCE_LEVELS
                                    .iter()
                                    .position(|level| *level == PerformanceLevel::Auto)
                                    .map(|index| index as u32),
                                options: PERFORMANCE_LEVELS
                                    .iter()
                                    .copied()
                                    .map(performance_option)
                                    .collect(),
                                selected: PERFORMANCE_LEVELS
                                    .iter()
                                    .position(|candidate| *candidate == level)
                                    .map(|index| index as u32),
                            }
                            .into(),
                            ..Default::default()
                        },
                    );
                }

                self.supported_power_mizer_modes = stats
                    .supported_power_mizer_modes
                    .clone()
                    .unwrap_or_default();
                if let Some(active) = stats.active_power_mizer_mode {
                    self.rows.insert(
                        PowerSetting::PowerMizer,
                        SettingRowInit {
                            title: fl!(I18N, "power-mizer-mode"),
                            control: SelectInit {
                                default_value: self
                                    .supported_power_mizer_modes
                                    .iter()
                                    .position(|mode| *mode == PowerMizerMode::Auto)
                                    .map(|index| index as u32),
                                options: self
                                    .supported_power_mizer_modes
                                    .iter()
                                    .copied()
                                    .map(power_mizer_option)
                                    .collect(),
                                selected: self
                                    .supported_power_mizer_modes
                                    .iter()
                                    .position(|mode| *mode == active)
                                    .map(|index| index as u32),
                            }
                            .into(),
                            ..Default::default()
                        },
                    );
                }

                self.power = power.clone();
                // Factory roots enter the list before init_widgets adds their CSS classes.
                self.rows.widget().invalidate_sort();
                self.performance_level_changed(&sender);
            }
            // Config arrives before the initial stats rebuild; it is not a user edit.
            PowerFrameMsg::NvidiaMode(mode) => self.nvidia_mode = mode,
            PowerFrameMsg::ToggleNvidiaIoctl(enabled) => {
                let mode = if enabled {
                    NvidiaPowerCapMode::Ioctl
                } else {
                    NvidiaPowerCapMode::Nvml
                };
                if self.nvidia_mode_available() && self.nvidia_mode != Some(mode) {
                    self.nvidia_mode = Some(mode);
                    if let Some(row) = self.rows.get(&PowerSetting::PowerCap)
                        && let SettingControl::Adjustment(control) = &row.control
                    {
                        control.emit(AdjustmentControlMsg::SetBounds {
                            lower: self.cap_min(self.power.cap_min),
                            upper: self.power.cap_max.unwrap_or_default(),
                        });
                    }
                    APP_BROKER.send(AppMsg::SettingsChanged);
                }
            }
            PowerFrameMsg::SetPerformanceLevel(level) => {
                if self.rows.get(&PowerSetting::PerformanceLevel).is_some()
                    && let Some(index) = PERFORMANCE_LEVELS
                        .iter()
                        .position(|candidate| *candidate == level)
                {
                    self.rows.send(
                        &PowerSetting::PerformanceLevel,
                        SettingRowMsg::SetValue(SettingRowValue::Selected(index as u32)),
                    );
                }
            }
            PowerFrameMsg::PowerProfileModes(table) => {
                if let Some(power_profile_mode) = self.power_profile_mode.take() {
                    self.rows.widget().remove(power_profile_mode.widget());
                }
                self.power_profile_mode = table.map(|table| {
                    AmdPowerProfileMode::builder()
                        .attach_to(self.rows.widget())
                        .launch((table, self.performance_level()))
                        .forward(APP_BROKER.sender(), |()| AppMsg::SettingsChanged)
                });
                self.rows.widget().invalidate_sort();
            }
            PowerFrameMsg::SettingChanged(setting) => {
                APP_BROKER.send(AppMsg::SettingsChanged);
                if setting == PowerSetting::PerformanceLevel {
                    self.performance_level_changed(&sender);
                }
            }
            PowerFrameMsg::Reset => {
                for setting in self.rows.keys() {
                    self.rows.send(setting, SettingRowMsg::Reset);
                }
            }
        }

        self.update_view(widgets, sender);
    }
}

impl PowerFrame {
    pub fn serialize(&self) -> PowerFrameSerialized {
        let value = |setting| self.rows.get(&setting).and_then(SettingRow::serialize);
        let selected = |setting| value(setting).and_then(SettingRowValue::as_selected);
        let power_profile_mode = self
            .power_profile_mode
            .as_ref()
            .and_then(|power_profile_mode| power_profile_mode.model().serialize());
        PowerFrameSerialized {
            power_cap: value(PowerSetting::PowerCap).and_then(SettingRowValue::as_numeric),
            nvidia_power_cap_mode: self.nvidia_power_cap_mode(),
            performance_level: selected(PowerSetting::PerformanceLevel)
                .and_then(|index| PERFORMANCE_LEVELS.get(index as usize).copied()),
            power_mizer_mode: selected(PowerSetting::PowerMizer).and_then(|index| {
                self.supported_power_mizer_modes
                    .get(index as usize)
                    .copied()
            }),
            power_profile_mode_index: power_profile_mode.as_ref().map(|mode| mode.index),
            custom_power_profile_mode_hueristics: power_profile_mode
                .map(|mode| mode.custom_heuristics)
                .unwrap_or_default(),
        }
    }

    fn nvidia_mode_available(&self) -> bool {
        self.nvidia_mode.is_some()
            && self.power.cap_min.is_some()
            && self.rows.get(&PowerSetting::PowerCap).is_some()
    }

    /// The native minimum, lowered to 30 W while the experimental mode is selected.
    fn cap_min(&self, native: Option<f64>) -> f64 {
        let native = native.unwrap_or_default();
        if self.nvidia_mode == Some(NvidiaPowerCapMode::Ioctl) {
            native.min(30.0)
        } else {
            native
        }
    }

    pub fn nvidia_power_cap_mode(&self) -> Option<NvidiaPowerCapMode> {
        self.nvidia_mode.filter(|_| self.nvidia_mode_available())
    }

    fn selected(&self, setting: PowerSetting) -> Option<u32> {
        self.rows.get(&setting)?.get_value().as_selected()
    }

    pub fn performance_level(&self) -> Option<PerformanceLevel> {
        PERFORMANCE_LEVELS
            .get(self.selected(PowerSetting::PerformanceLevel)? as usize)
            .copied()
    }

    fn performance_level_changed(&self, sender: &ComponentSender<Self>) {
        if let Some(power_profile_mode) = &self.power_profile_mode {
            power_profile_mode.emit(AmdPowerProfileModeMsg::PerformanceLevel(
                self.performance_level(),
            ));
        }
        sender.output(OcPageMsg::PerformanceLevelChanged).unwrap();
    }
}

fn performance_option(level: PerformanceLevel) -> SelectOption {
    let (label, description) = match level {
        PerformanceLevel::Auto => (
            fl!(I18N, "performance-level-auto"),
            fl!(I18N, "performance-level-auto-description"),
        ),
        PerformanceLevel::High => (
            fl!(I18N, "performance-level-high"),
            fl!(I18N, "performance-level-high-description"),
        ),
        PerformanceLevel::Low => (
            fl!(I18N, "performance-level-low"),
            fl!(I18N, "performance-level-low-description"),
        ),
        PerformanceLevel::Manual => (
            fl!(I18N, "performance-level-manual"),
            fl!(I18N, "performance-level-manual-description"),
        ),
        PerformanceLevel::ProfileStandard => (
            fl!(I18N, "performance-level-profile-standard"),
            fl!(I18N, "performance-level-profile-standard-description"),
        ),
        PerformanceLevel::ProfileMinSclk => (
            fl!(I18N, "performance-level-profile-min-sclk"),
            fl!(I18N, "performance-level-profile-min-sclk-description"),
        ),
        PerformanceLevel::ProfileMinMclk => (
            fl!(I18N, "performance-level-profile-min-mclk"),
            fl!(I18N, "performance-level-profile-min-mclk-description"),
        ),
        PerformanceLevel::ProfilePeak => (
            fl!(I18N, "performance-level-profile-peak"),
            fl!(I18N, "performance-level-profile-peak-description"),
        ),
        PerformanceLevel::PerfDeterminism | PerformanceLevel::ProfileExit => {
            (level.to_string(), String::new())
        }
    };
    SelectOption { label, description }
}

fn power_mizer_option(mode: PowerMizerMode) -> SelectOption {
    let (label, description) = match mode {
        PowerMizerMode::Auto => (
            fl!(I18N, "power-mizer-mode-auto"),
            fl!(I18N, "power-mizer-mode-auto-description"),
        ),
        PowerMizerMode::Adaptive => (
            fl!(I18N, "power-mizer-mode-adaptive"),
            fl!(I18N, "power-mizer-mode-adaptive-description"),
        ),
        PowerMizerMode::PreferMaximumPerformance => (
            fl!(I18N, "power-mizer-mode-prefer-maximum-performance"),
            fl!(
                I18N,
                "power-mizer-mode-prefer-maximum-performance-description"
            ),
        ),
        PowerMizerMode::PreferConsistentPerformance => (
            fl!(I18N, "power-mizer-mode-prefer-consistent-performance"),
            fl!(
                I18N,
                "power-mizer-mode-prefer-consistent-performance-description"
            ),
        ),
    };
    SelectOption { label, description }
}

#[cfg(all(test, feature = "gtk-tests"))]
#[path = "power_frame_tests.rs"]
mod tests;
