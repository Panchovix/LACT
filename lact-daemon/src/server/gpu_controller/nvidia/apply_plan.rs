//! Pure planning and success-only bookkeeping for independent NVIDIA settings.
use super::{
    driver::{ClockDomainOffset, ClockDomainState},
    nvapi::NvGpuClockDomainId,
};
use lact_schema::config::GpuConfig;
use std::cell::Cell;

/// Preserve freshly read voltage demand while removing only frequency offsets.
/// Zero can increase a clock whose previous offset was negative; this is a
/// software compensation order, not a claim about physical frequency or safety.
pub(super) fn frequency_reset_offsets(states: Vec<ClockDomainState>) -> Vec<ClockDomainOffset> {
    states
        .into_iter()
        .filter(|state| NvGpuClockDomainId::from_id(state.domain).is_some())
        .map(|state| ClockDomainOffset {
            domain: state.domain,
            freq_offset_khz: 0,
            msvdd_offset_uv: state.msvdd_offset_uv,
        })
        .collect()
}

pub(super) fn frequency_reset_required(
    written: bool,
    discovery: bool,
    capability_present: bool,
) -> bool {
    written || (discovery && capability_present)
}

pub(super) fn verify_frequency_reset(
    expected: &[ClockDomainOffset],
    actual: &[ClockDomainState],
) -> anyhow::Result<()> {
    for offset in expected {
        let matches: Vec<_> = actual
            .iter()
            .filter(|state| state.domain == offset.domain)
            .collect();
        anyhow::ensure!(
            matches.len() == 1,
            "Compensated clock domain {} has no unique readback",
            offset.domain
        );
        let state = matches[0];
        anyhow::ensure!(
            state.freq_offset_khz == 0 && state.msvdd_offset_uv == offset.msvdd_offset_uv,
            "Clock domain {} compensation did not preserve frequency/voltage demand readback",
            offset.domain
        );
    }
    Ok(())
}

pub(super) fn recover_frequency_if_needed(
    needed: &Cell<bool>,
    recover: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if needed.get() {
        recover()?;
        needed.set(false);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Group {
    PowerCap,
    PowerMizer,
    Clocks,
    Boost,
    Rails,
    Fans,
    Thermal,
}

impl Group {
    const ALL: [Self; 7] = [
        Self::PowerCap,
        Self::PowerMizer,
        Self::Clocks,
        Self::Boost,
        Self::Rails,
        Self::Fans,
        Self::Thermal,
    ];

    /// Applied first: their writes do not depend on rail policies, so a later
    /// rail or clock failure cannot leave them unapplied.
    const INDEPENDENT: [Self; 4] = [Self::PowerCap, Self::PowerMizer, Self::Fans, Self::Thermal];

    /// Rail policies settle before new boost and clock settings, in this order.
    pub(super) const VOLTAGE_COUPLED: [Self; 3] = [Self::Boost, Self::Rails, Self::Clocks];

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    ResetClocks,
    Apply(Group),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ApplyPlan {
    groups: u8,
}

impl ApplyPlan {
    pub(super) fn includes(self, group: Group) -> bool {
        self.groups & group.bit() != 0
    }

    /// The groups that remain applicable without a usable runtime helper.
    pub(super) fn independent_groups(self) -> Vec<Group> {
        Group::INDEPENDENT
            .into_iter()
            .filter(|group| self.includes(*group))
            .collect()
    }

    /// The clock reset and voltage-coupled groups, in the order they are written.
    pub(super) fn voltage_steps(self) -> Vec<Step> {
        let mut steps = Vec::new();
        if self.includes(Group::Clocks) {
            steps.push(Step::ResetClocks);
        }
        for group in Group::VOLTAGE_COUPLED {
            if self.includes(group) {
                steps.push(Step::Apply(group));
            }
        }
        steps
    }

    /// Every step in Apply order: the independent groups, then `voltage_steps`.
    #[cfg(test)]
    fn steps(self) -> Vec<Step> {
        let mut steps: Vec<_> = self
            .independent_groups()
            .into_iter()
            .map(Step::Apply)
            .collect();
        steps.extend(self.voltage_steps());
        steps
    }
}

#[derive(Default)]
pub(super) struct AppliedState {
    last_successful: Option<GpuConfig>,
    /// The latest configuration whose voltage-coupled groups wait for the
    /// helper, or the one an edit replaces when nothing was applied yet.
    pending_voltage: Option<GpuConfig>,
    uncertain: u8,
}

impl AppliedState {
    pub(super) fn plan(&self, desired: &GpuConfig) -> ApplyPlan {
        let mut plan = ApplyPlan {
            groups: self.uncertain,
        };
        for group in Group::ALL {
            if self
                .last_successful
                .as_ref()
                .is_none_or(|previous| !equal(group, previous, desired))
            {
                plan.groups |= group.bit();
            }
        }
        if plan.includes(Group::Clocks) {
            // A clock reset must be followed by the desired voltage settings
            // before writing the new frequency offsets/curve/ratio.
            plan.groups |= Group::Boost.bit() | Group::Rails.bit();
        }
        if desired.nvidia_rail_controls.is_none() {
            // The helper owns durable state independently of this controller.
            // None must still release any ownership, including after a restart.
            plan.groups |= Group::Rails.bit();
        }
        plan
    }

    pub(super) fn attempted(&mut self, group: Group) {
        self.uncertain |= group.bit();
    }

    pub(super) fn complete(&mut self, config: &GpuConfig) {
        self.last_successful = Some(config.clone());
        self.pending_voltage = None;
        self.uncertain = 0;
    }

    pub(super) fn invalidate(&mut self, group: Group) {
        self.uncertain |= group.bit();
    }

    pub(super) fn invalidate_all(&mut self) {
        self.last_successful = None;
        self.pending_voltage = None;
        self.uncertain = 0;
    }

    /// With nothing applied or waiting yet, an edit is compared with the
    /// configuration it replaces, so new boost, rail or clock settings in it
    /// count as a change instead of becoming the reference.
    pub(super) fn note_previous(&mut self, previous: &GpuConfig) {
        if self.last_successful.is_none() && self.pending_voltage.is_none() {
            self.pending_voltage = Some(previous.clone());
        }
    }

    /// Starts an Apply without a usable helper: the independent groups to write
    /// now are marked attempted, and boost, rails and clocks are left pending.
    pub(super) fn begin_without_helper(&mut self, desired: &GpuConfig) -> DegradedApply {
        let groups = self.plan(desired).independent_groups();
        let new_voltage_request = self.defer_voltage_groups(desired);
        for group in &groups {
            self.attempted(*group);
        }
        DegradedApply {
            groups,
            new_voltage_request,
        }
    }

    /// Returns whether `desired` asks for boost, rail or clock settings other
    /// than the ones applied or already waiting, which cannot take effect yet.
    /// Only settings that may take effect later replace the waiting ones, so
    /// reverting a rejected request compares against what was there before it.
    fn defer_voltage_groups(&mut self, desired: &GpuConfig) -> bool {
        let baseline = self
            .pending_voltage
            .as_ref()
            .or(self.last_successful.as_ref());
        let changed = baseline.is_some_and(|baseline| {
            Group::VOLTAGE_COUPLED
                .into_iter()
                .any(|group| !equal(group, baseline, desired))
        });
        for group in Group::VOLTAGE_COUPLED {
            self.uncertain |= group.bit();
        }
        // With nothing known, as when startup or a profile switch applies an
        // existing configuration, that configuration is the best reference for
        // later changes. Edits note the configuration they replace first.
        if !changed {
            self.pending_voltage = Some(desired.clone());
        }
        changed
    }
}

pub(super) struct DegradedApply {
    pub(super) groups: Vec<Group>,
    pub(super) new_voltage_request: bool,
}

fn equal(group: Group, previous: &GpuConfig, desired: &GpuConfig) -> bool {
    match group {
        Group::PowerCap => {
            previous.power_cap == desired.power_cap
                && previous.nvidia_power_cap_mode == desired.nvidia_power_cap_mode
        }
        Group::PowerMizer => previous.power_mizer_mode == desired.power_mizer_mode,
        Group::Clocks => {
            let mut before = previous.clocks_configuration.clone();
            let mut after = desired.clocks_configuration.clone();
            before.voltage_boost = None;
            after.voltage_boost = None;
            before == after
        }
        Group::Boost => {
            previous.clocks_configuration.voltage_boost
                == desired.clocks_configuration.voltage_boost
        }
        Group::Rails => previous.nvidia_rail_controls == desired.nvidia_rail_controls,
        Group::Fans => {
            previous.fan_control_enabled == desired.fan_control_enabled
                && previous.fan_control_settings == desired.fan_control_settings
        }
        Group::Thermal => previous.nvidia_thermal_options == desired.nvidia_thermal_options,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lact_schema::{NvidiaRailControls, NvidiaRailSettings, config::NvidiaPowerCapMode};

    fn configured() -> GpuConfig {
        GpuConfig {
            power_cap: Some(250.0),
            nvidia_power_cap_mode: NvidiaPowerCapMode::Ioctl,
            nvidia_rail_controls: Some(NvidiaRailControls {
                nvvdd: NvidiaRailSettings {
                    max_voltage_uv: Some(1125000),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn groups(plan: ApplyPlan) -> Vec<Group> {
        Group::ALL
            .into_iter()
            .filter(|group| plan.includes(*group))
            .collect()
    }

    #[test]
    fn startup_and_controller_reload_apply_all_groups() {
        let config = configured();
        let mut state = AppliedState::default();
        assert_eq!(groups(state.plan(&config)), Group::ALL);
        state.complete(&config);
        assert!(groups(state.plan(&config)).is_empty());
        state.invalidate_all();
        assert_eq!(groups(state.plan(&config)), Group::ALL);
    }

    #[test]
    fn power_and_rail_edits_leave_clocks_boost_fans_and_thermal_untouched() {
        let original = configured();
        let mut state = AppliedState::default();
        state.complete(&original);
        let mut power = original.clone();
        power.power_cap = Some(230.0);
        assert_eq!(groups(state.plan(&power)), [Group::PowerCap]);
        power.power_cap = original.power_cap;
        power.nvidia_power_cap_mode = NvidiaPowerCapMode::Nvml;
        assert_eq!(groups(state.plan(&power)), [Group::PowerCap]);
        let mut rail = original.clone();
        rail.nvidia_rail_controls
            .as_mut()
            .unwrap()
            .nvvdd
            .max_voltage_uv = Some(1100000);
        assert_eq!(groups(state.plan(&rail)), [Group::Rails]);
        rail.nvidia_rail_controls = None;
        assert_eq!(groups(state.plan(&rail)), [Group::Rails]);
        state.complete(&rail);
        assert_eq!(groups(state.plan(&rail)), [Group::Rails]);
    }

    #[test]
    fn clock_changes_require_voltage_groups_but_boost_changes_do_not_reset_clocks() {
        let original = configured();
        let mut state = AppliedState::default();
        state.complete(&original);
        let mut changed = original.clone();
        changed
            .clocks_configuration
            .gpu_clock_offsets
            .insert(0, 100);
        assert_eq!(
            groups(state.plan(&changed)),
            [Group::Clocks, Group::Boost, Group::Rails]
        );
        changed = original.clone();
        changed.clocks_configuration.voltage_boost = Some(25);
        assert_eq!(groups(state.plan(&changed)), [Group::Boost]);
    }

    #[test]
    fn partial_attempts_force_previous_configuration_to_reapply_until_success() {
        let original = configured();
        let mut state = AppliedState::default();
        state.complete(&original);
        state.attempted(Group::PowerCap);
        state.attempted(Group::Rails);
        assert_eq!(
            groups(state.plan(&original)),
            [Group::PowerCap, Group::Rails]
        );
        // Repeated recovery failure cannot accidentally clear the uncertainty.
        state.attempted(Group::Rails);
        assert_eq!(
            groups(state.plan(&original)),
            [Group::PowerCap, Group::Rails]
        );
        state.complete(&original);
        assert!(groups(state.plan(&original)).is_empty());
    }

    #[test]
    fn explicit_clock_reset_invalidates_clocks_and_boost_without_unrelated_groups() {
        let original = configured();
        let mut state = AppliedState::default();
        state.complete(&original);
        state.invalidate(Group::Clocks);
        state.invalidate(Group::Boost);
        assert_eq!(
            groups(state.plan(&original)),
            [Group::Clocks, Group::Boost, Group::Rails]
        );
    }

    #[test]
    fn voltage_settings_precede_new_clock_offsets_after_reset() {
        let original = configured();
        let mut state = AppliedState::default();
        state.complete(&original);
        let mut changed = original.clone();
        changed
            .clocks_configuration
            .mem_clock_offsets
            .insert(0, 100);
        assert_eq!(
            state.plan(&changed).steps(),
            [
                Step::ResetClocks,
                Step::Apply(Group::Boost),
                Step::Apply(Group::Rails),
                Step::Apply(Group::Clocks),
            ]
        );
        changed = original.clone();
        changed.power_cap = Some(230.0);
        assert_eq!(state.plan(&changed).steps(), [Step::Apply(Group::PowerCap)]);
    }

    #[test]
    fn every_failed_group_remains_uncertain_and_rollback_skips_unattempted_groups() {
        let original = configured();
        for failed in Group::ALL {
            let mut state = AppliedState::default();
            state.complete(&original);
            let mut desired = original.clone();
            desired.power_cap = Some(230.0);
            desired.power_mizer_mode =
                Some(nvml_wrapper::enums::device::PowerMizerMode::PreferMaximumPerformance);
            desired
                .clocks_configuration
                .gpu_clock_offsets
                .insert(0, 100);
            desired.clocks_configuration.voltage_boost = Some(25);
            desired
                .nvidia_rail_controls
                .as_mut()
                .unwrap()
                .nvvdd
                .max_voltage_uv = Some(1100000);
            desired.fan_control_enabled = true;
            desired.nvidia_thermal_options.target_temperature = Some(70);
            let mut attempted = Vec::new();
            for step in state.plan(&desired).steps() {
                let group = match step {
                    Step::ResetClocks => Group::Clocks,
                    Step::Apply(group) => group,
                };
                state.attempted(group);
                if !attempted.contains(&group) {
                    attempted.push(group);
                }
                if group == failed {
                    break;
                }
            }
            let recovery = state.plan(&original);
            for group in Group::ALL {
                // A clock reset also brings back the voltage groups it precedes.
                let forced = attempted.contains(&Group::Clocks)
                    && matches!(group, Group::Boost | Group::Rails);
                assert_eq!(
                    recovery.includes(group),
                    attempted.contains(&group) || forced,
                    "{failed:?} failure: wrong {group:?} recovery"
                );
            }
            // The earlier success snapshot survives the failed execution.
            assert_eq!(state.last_successful.as_ref(), Some(&original));
            state.complete(&original);
            assert!(groups(state.plan(&original)).is_empty());
        }
    }

    #[test]
    fn independent_groups_are_applied_before_the_voltage_coupled_ones() {
        let state = AppliedState::default();
        assert_eq!(
            state.plan(&configured()).steps(),
            [
                Step::Apply(Group::PowerCap),
                Step::Apply(Group::PowerMizer),
                Step::Apply(Group::Fans),
                Step::Apply(Group::Thermal),
                Step::ResetClocks,
                Step::Apply(Group::Boost),
                Step::Apply(Group::Rails),
                Step::Apply(Group::Clocks),
            ]
        );
    }

    #[test]
    fn unusable_helper_applies_independent_groups_and_keeps_voltage_groups_pending() {
        let original = configured();
        let mut state = AppliedState::default();
        state.complete(&original);
        let mut desired = original.clone();
        desired.power_cap = Some(230.0);
        // The daemon applies only these while the helper cannot run.
        let degraded = state.begin_without_helper(&desired);
        assert_eq!(degraded.groups, [Group::PowerCap]);
        assert!(!degraded.new_voltage_request);
        // Unchanged voltage settings are still applied once the helper works.
        assert_eq!(
            state.plan(&desired).steps(),
            [
                Step::Apply(Group::PowerCap),
                Step::ResetClocks,
                Step::Apply(Group::Boost),
                Step::Apply(Group::Rails),
                Step::Apply(Group::Clocks),
            ]
        );
        state.complete(&desired);
        assert!(groups(state.plan(&desired)).is_empty());
    }

    #[test]
    fn degraded_applies_tell_new_voltage_requests_from_ones_already_waiting() {
        let original = configured();
        let mut state = AppliedState::default();
        let mut new_request = |config: &GpuConfig| {
            let degraded = state.begin_without_helper(config);
            degraded.new_voltage_request
        };
        // Startup with a broken helper: the configuration becomes the reference.
        assert!(!new_request(&original));
        let mut power = original.clone();
        power.power_cap = Some(230.0);
        assert!(!new_request(&power));
        let mut clocks = power.clone();
        clocks.clocks_configuration.mem_clock_offsets.insert(0, 100);
        assert!(new_request(&clocks));
        // Reverting the rejected change finds the earlier reference intact.
        assert!(!new_request(&power));
        let mut rails = power.clone();
        rails.nvidia_rail_controls = None;
        assert!(new_request(&rails));
        assert!(!new_request(&power));
        // A complete Apply makes the applied settings the reference.
        state.complete(&original);
        let mut new_request = |config: &GpuConfig| {
            let degraded = state.begin_without_helper(config);
            degraded.new_voltage_request
        };
        assert!(!new_request(&power));
        assert!(new_request(&clocks));
        state.invalidate_all();
        assert!(!state.begin_without_helper(&clocks).new_voltage_request);
    }

    #[test]
    fn an_edit_is_compared_with_the_configuration_it_replaces() {
        let mut state = AppliedState::default();
        // The first edit of a GPU without saved settings, during an outage.
        state.note_previous(&GpuConfig::default());
        let power = GpuConfig {
            power_cap: Some(230.0),
            ..Default::default()
        };
        let mut rails = power.clone();
        rails.nvidia_rail_controls = configured().nvidia_rail_controls;
        let mut clocks = power.clone();
        clocks.clocks_configuration.mem_clock_offsets.insert(0, 100);
        assert!(state.begin_without_helper(&rails).new_voltage_request);
        assert!(state.begin_without_helper(&clocks).new_voltage_request);
        assert!(!state.begin_without_helper(&power).new_voltage_request);
        // A note never replaces a reference that is already known.
        state.note_previous(&configured());
        assert!(state.begin_without_helper(&rails).new_voltage_request);
    }

    #[test]
    fn warned_or_released_groups_are_retried_alone_with_unchanged_settings() {
        let original = configured();
        let mut state = AppliedState::default();
        for retried in [Group::Boost, Group::Thermal, Group::Fans] {
            state.complete(&original);
            state.invalidate(retried);
            assert_eq!(state.plan(&original).steps(), [Step::Apply(retried)]);
        }
    }

    #[test]
    fn frequency_compensation_preserves_fresh_msvdd_demands_in_sparse_domains() {
        let states = [
            (1, 120000, 25000),
            (2, -50000, -12500),
            (21, 90000, 40000),
            (0, 100000, 33333),
        ]
        .into_iter()
        .map(
            |(domain, freq_offset_khz, msvdd_offset_uv)| ClockDomainState {
                domain,
                freq_offset_khz,
                msvdd_offset_uv,
                min_offset_mhz: -100,
                max_offset_mhz: 200,
            },
        )
        .collect();
        let offsets = frequency_reset_offsets(states);
        let values: Vec<_> = offsets
            .into_iter()
            .map(|offset| {
                (
                    offset.domain,
                    offset.freq_offset_khz,
                    offset.msvdd_offset_uv,
                )
            })
            .collect();
        assert_eq!(values, [(1, 0, 25000), (2, 0, -12500), (21, 0, 40000)]);
    }

    #[test]
    fn failed_frequency_compensation_blocks_rail_rollback_and_is_retried() {
        let needed = Cell::new(true);
        let mut operations = Vec::new();
        let failed: anyhow::Result<()> = (|| {
            recover_frequency_if_needed(&needed, || {
                operations.push("frequency compensation");
                anyhow::bail!("synthetic native failure")
            })?;
            operations.push("rail rollback");
            Ok(())
        })();
        assert!(failed.is_err());
        assert!(needed.get());
        assert_eq!(operations, ["frequency compensation"]);
        recover_frequency_if_needed(&needed, || {
            operations.push("frequency retry");
            Ok(())
        })
        .unwrap();
        operations.push("rail rollback");
        assert!(!needed.get());
        assert_eq!(
            operations,
            ["frequency compensation", "frequency retry", "rail rollback"]
        );
        recover_frequency_if_needed(&needed, || {
            anyhow::bail!("must not retry completed compensation")
        })
        .unwrap();
    }

    #[test]
    fn unknown_checkpoint_discovers_frequency_groups_despite_empty_controller_flags() {
        assert!(!frequency_reset_required(false, false, true));
        assert!(frequency_reset_required(false, true, true));
        assert!(!frequency_reset_required(false, true, false));
        assert!(frequency_reset_required(true, false, true));
        let needed = Cell::new(true);
        let discovery = Cell::new(true);
        let mut operations = Vec::new();
        let result: anyhow::Result<()> = (|| {
            recover_frequency_if_needed(&needed, || {
                assert!(frequency_reset_required(false, discovery.get(), true));
                operations.push("discover frequency settings");
                anyhow::bail!("synthetic discovery GET failure")
            })?;
            operations.push("rollback unknown rail checkpoint");
            Ok(())
        })();
        assert!(result.is_err());
        assert!(needed.get());
        assert!(discovery.get());
        assert_eq!(operations, ["discover frequency settings"]);
    }

    #[test]
    fn altered_or_missing_domain_compensation_readback_refuses_rail_rollback() {
        let expected = [ClockDomainOffset {
            domain: 1,
            freq_offset_khz: 0,
            msvdd_offset_uv: 25000,
        }];
        let original = ClockDomainState {
            domain: 1,
            freq_offset_khz: 0,
            msvdd_offset_uv: 25000,
            min_offset_mhz: -100,
            max_offset_mhz: 200,
        };
        assert!(verify_frequency_reset(&expected, &[original]).is_ok());
        assert!(verify_frequency_reset(&expected, &[]).is_err());
        assert!(verify_frequency_reset(&expected, &[original, original]).is_err());
        assert!(
            verify_frequency_reset(
                &expected,
                &[ClockDomainState {
                    freq_offset_khz: 50000,
                    ..original
                }]
            )
            .is_err()
        );
        assert!(
            verify_frequency_reset(
                &expected,
                &[ClockDomainState {
                    msvdd_offset_uv: 0,
                    ..original
                }]
            )
            .is_err()
        );
    }
}
