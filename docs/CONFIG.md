# Configuration

The LACT config file is located in `/etc/lact/config.yaml`, and contains all of the GPU settings that are typically edited in the GUI, as well as a few settings specifying the behaviour of the daemon.
LACT listens for config file changes and reloads all GPU settings automatically, but daemon-related settings such as the logging level or permissions require a service restart (`systemctl restart lactd`).

Full config file with all possible options:
```yaml
# WARNING: this is only an example of each possible setting. DO NOT COPY THIS CONFIG AS IS.
# Many options don't make sense to be used together, and depend on your hardware.
daemon:
  # The logging level of the daemon.
  # Possible values: `error`, `warn`, `info` (default), `debug`, `trace`
  log_level: info
  # User group that owns the daemon socket.
  # Any user in this group will be able to use the daemon.
  # Access can also be granted with the `admin_user` setting. 
  admin_group: wheel
  # User that owns the daemon socket.
  # This user will have access to the daemon, even if they are not in the part of the `admin_group` group.    
  # Not set by default.
  admin_user: foo
  # If set to `true`, this setting makes the LACT daemon not reset
  # GPU clocks when changing other settings or when turning off the daemon.
  # Can be used to work around a few very specific issues with 
  # some settings not applying on AMD GPUs.
  disable_clocks_cleanup: false
  # Disables fetching of additional information through NvAPI on Nvidia GPUs.
  # Not set by default.
  disable_nvapi: false
  # Optional external NVIDIA rail-control provider. Disabled unless configured.
  # Install the provider separately in an administrator-owned prefix.
  # Changing these daemon options requires a service restart.
  # nvidia_runtime:
  #   executable: /opt/nvidia-runtime-tools/bin/nvidia-runtime
  #   state_dir: /var/lib/lact/nvidia-runtime
  #   timeout_ms: 10000
  # Daemon's TCP listening address. Not specified by default.
  # By default TCP access is disabled, and only a unix socket is present.
  # Specifying this option enables the TCP listener.
  tcp_listen_address: 127.0.0.1:12853
  # Opentelemetry metrics exporter configuration
  # Omitted by default, meaning the exporter is disabled.
  metrics:
    # Opentelemetry collector URL. The following example is for local Prometheus.
    collector_address: 'http://localhost:9090/api/v1/otlp/v1/metrics'
    # Interval in seconds for how often to collect the metrics. 
    # Defaults to 30.
    interval: 30

# Period in seconds for how long settings should wait to be confirmed.
# Most GPU setting change commands require a confirmation command to be used
# in order to save these settings to the config. 
# If a confirm command is not issued within the configured period (default: 5 seconds)
# the setting will be reverted.
apply_settings_timer: 5

# The main GPU configuration map, containing the list of GPUs and their settings.
gpus:
  # A GPU config entry. This is the ID of the GPU.
  # The ID is formed with a combination of a PCI device id, 
  # PCI subsystem id and PCI slot name to uniquely identify 
  # each GPU in the system, even if there are multiple of the same model.

  # You can discover the id of your GPU by either:
  # - Changing a setting in the UI, so it's written to the config
  # - Using `lact cli list-gpus`
  1002:687F-1043:0555-0000:0b:00.0:
    # Whether the daemon should touch fan control settings at all.
    # Setting this to `true` requires the `fan_control_settings` field to be present as well.
    fan_control_enabled: true
    fan_control_settings:
      # Fan control mode. Can be either `curve` or `static`
      mode: curve
      # Static fan speed from 0 to 1. Used when `mode` is `static`
      static_speed: 1.0
      # The temperature sensor name to be used with a custom fan curve.
      # This can be used to base the fan curve off  the`junction` (hotspot) 
      # temperature instead of the default overall ("edge") tempreature.
      # Applicable on most Vega and newer AMD GPUs.
      temperature_key: edge
      # Interval in milliseconds for how often the GPU temperature should be checked
      # when adjusting the fan curve.
      interval_ms: 500
      # Custom fan curve used with `mode` set to `curve`.
      # The format of the map is temperature to fan speed from 0 to 1.
      # Note: on RDNA3+ AMD GPUs this must have 5 entries.
      curve:
        40: 0.2
        50: 0.35
        60: 0.5
        70: 0.75
        80: 1.0
      # Hysteresis setting: when spinning down fans after a temperature drop,
      # the target speed needs to be lower for at least this many milliseconds
      # for the fan to actually slow down.
      # This lets you avoid fan speed jumping around during short drops of load
      # (e.g. loading screen in a game).
      spindown_delay_ms: 0
      # Hysteresis setting: the minimum temperature change in degrees 
      # to affect the fan speed. Also used to avoid rapid fan speed changes
      # when the temperature only changes e.g. 1 degree.
      change_threshold: 0
      # A temperature below which the fan control mode is switched to automatic (Nvidia only)
      # This can be used as a workaround to achieve 0 RPM below a certain temperature even when the GPU only allows speeds like 30-100% to be set manually.
      auto_threshold: 0
    # Power management firmware options. Specific to RDNA3+ AMD GPUs.
    # Most of these settings are only applied when not using a custom fan curve.
    pmfw_options: 
      # This setting adjusts the PMFW’s behavior about the maximum speed in RPM the fan can spin.
      acoustic_limit: 3200
      # This setting adjusts the PMFW’s behavior about the maximum speed in RPM the fan can spin 
      # when the temperature is not greater than target temperature.
      acoustic_target: 1450
      # The minimum speed in RPM that the fan can spin at.
      minimum_pwm: 15
      # Target temperature for the GPU in degrees.
      # Paring with the acoustic_target setting, they define the maximum speed in RPM 
      # the fan can spin when the temperature is not greater than target temperature. 
      target_temperature: 83
      # When set to `true`, allows the fan to be turned turned off when below the
      # `zero_rpm_threshold` temperature value.
      zero_rpm: true
      # Temperature in degrees below which the fan should be turned off when `zero_rpm` is set to true.
      zero_rpm_threshold: 50
    # Nvidia thermal options.
    nvidia_thermal_options:
      # Target temperature for the GPU in degrees.
      target_temperature: 83
    # Power limit in watts.
    power_cap: 320.0
    # NVIDIA only: `nvml` (default) or `ioctl`, an experimental undocumented interface
    # that applies every power limit and allows values down to 30 W.
    nvidia_power_cap_mode: nvml
    # Performance level option for AMD GPUs.
    # Can be `auto`, `low`, `high` or `manual`.
    performance_level: auto
    # PowerMizer mode for Nvidia GPUs.
    # Can be `Auto`, `Adaptive`, `PreferMaximumPerformance` or `PreferConsistentPerformance`.
    power_mizer_mode: Auto
    # Index of an AMD power profile mode.
    # Setting this requires `performance_level` to be set to `manual`.
    power_profile_mode_index: 0
    # Custom heuristic values when using the custom AMD power profile mode.
    # The meaning of these values, their format and count depend on the specific GPU model.
    # Check the names of these values in the UI.
    custom_power_profile_mode_hueristics:
    - - 0
      - 5
      - 1
      - 0
      - 4
      - 800
      - 4587520
      - -65536
      - 0
    - - 0
      - 5
      - 1
      - 0
      - 1
      - 0
      - 3276800
      - -65536
      - -6553
    - - 0
      - 5
      - 1
      - 0
      - 4
      - 800
      - 327680
      - -65536
      - 0
    # List of AMD power states which should be enabled
    power_states:
      # GPU power states
      core_clock:
        - 0
        - 2
        - 3
      # VRAM power states
      memory_clock:
        - 0
        - 1
    
    ## Min/Max clock and voltage settings. On Nvidia, min and max values always have to be set together.
    # Minimum GPU clockspeed in MHz.
    min_core_clock: 300
    # Minimum VRAM clockspeed in MHz.
    min_memory_clock: 500
    # Minimum GPU voltage in mV. Applicable to AMD only.
    min_voltage: 900
    # Maximum GPU clockspeed in MHz.
    max_core_clock: 1630
    # Maximum VRAM clockspeed in MHz.
    max_memory_clock: 800
    # Maximum GPU voltage in mV. Applicable to Vega and earlier AMD GPUs.
    max_voltage: 1200
    # Voltage offset value in mV for RDNA and newer AMD GPUs.
    voltage_offset: 0
    # Nvidia core voltage boost, in percent (0-100) of available boost, not total voltage
    voltage_boost: 0

    # GPU V/F curve with voltage and frequency points per each power state.
    # Applicable only to AMD GCN and RDNA1 GPUs. Overrides the values of min/max clock/voltage fields.
    gpu_vf_curve:
      7:
        clockspeed: 1590
        voltage: 1200
    nvidia_gpu_vf_curve:
      22:
        clockspeed_offset: 90
        voltage: 750
    # VRAM V/F curve with voltage and frequency points per each power state.
    # Applicable only to AMD GCN GPUs. Overrides the values of min/max memory clock fields.
    mem_vf_curve:
      3:
        clockspeed: 920
    
    # GPU and VRAM clockspeed offset values, per-pstate. Applicable on Nvidia and on AMD RDNA4.
    gpu_clock_offsets:
      0: -100
    mem_clock_offsets:
      0: 200

    # Clockspeed offset values in MHz for the clock domains Nvidia does not expose
    # through its normal interface, keyed by domain id: 1 is XBAR, 2 is SYS and
    # 20 is NVD. Only the domains the card reports as adjustable are accepted.
    clock_domain_offsets:
      1: 500
    # Voltage offset values in mV for those same domains, keyed by domain id.
    # Each one shifts how much voltage its domain asks for at a given clockspeed,
    # and the domains share the MSVDD rail, so a positive offset lowers that
    # domain's clocks and a negative one raises them. Limited to +-50.
    clock_domain_voltage_offsets:
      1: -10
    # Ratio at which the GPC clock propagates to the XBAR domain, in percent,
    # 90 by default. Raising it decouples XBAR from the core clock without
    # changing the core clock itself. Applicable to Nvidia Blackwell and newer.
    xbar_ratio: 75

    # Index of the UMA (dedicated VRAM) configuration that should be used.
    # Needs a reboot to actually take effect. Applicable to certain AMD iGPUs.
    uma_carveout: 1

# Settings profiles
profiles:
  # Name of the profile
  vkcube:
    # GPU settings in this profile. 
    # It is the same config format that is used for the top-level `gpus` option.
    gpus: {}
    # Profile activation rule for when this profile should be activated
    # when using automatic profile switching.
    rule:
      # Type of the rule. Can be `process`, `gamemode`, `and`, or `or`.
      type: process
      # Process filter. This is not required when using the gamemode rule type.
      filter: # must be a list when type is `and` or `or`
        # Name of the process.
        name: vkcube
        # Process arguments. Not required.
        args: --my-arg
    # Hooks that run when the profile is activated or deactivated
    hooks:
      activated: echo foo >> /tmp/log
      deactivated: echo bar >> /tmp/log

# Current profile to be used. Does not have effect when `auto_switch_profiles` is used.
# Omit this option or set to `null` to use the default profile (settings in the top-level `gpus` entry).
current_profile: vkcube
# If profiles should be switched between automatically based on their configured rules.
auto_switch_profiles: true
```


## Optional NVIDIA rail controls

The optional external `nvidia-runtime` provider adds separate NVVDD and MSVDD
controls to the overclocking page. `Voltages` shows driver-reported voltage,
evaluated limits and directly editable VMIN/REL requests for each supported rail.
The separate `NVIDIA protections` section holds OV and OCP controls. LACT uses
a typed JSON interface; native implementation and verification data are maintained outside the LACT source tree.
Without provider configuration, LACT does not launch the provider. Unsupported
GPUs or driver versions remain unavailable rather than inheriting another GPU's
capabilities.

Install a provider release that supports protocol 1 and its `lact-bridge` command
in an administrator-owned prefix. The executable and its ancestors must not be
writable by other users. A root daemon will therefore reject an executable in a
user-owned checkout. For example, a separately installed Python environment may
provide `/opt/nvidia-runtime-tools/bin/nvidia-runtime`. Install the provider code
and dependencies under the same administrative ownership; do not point its
launcher at user-writable source files.

Create a private state directory and configure the daemon:

```yaml
daemon:
  nvidia_runtime:
    executable: /opt/nvidia-runtime-tools/bin/nvidia-runtime
    state_dir: /var/lib/lact/nvidia-runtime
    timeout_ms: 10000
```

The state directory's parent must already exist. The provider creates the final
directory with mode 0700, or requires an existing directory owned by the daemon
user with that mode. Keep this directory: deleting it does not restore GPU
settings. Changes to the helper path, directory or timeout require a daemon
restart, like other daemon options.

Voltage requests can be edited directly when the provider reports support.
OV and OCP editing has its own enable checkbox; turning it off releases only
those protection requests, preserving VMIN and REL. Edits take effect through
Apply and confirmation. Settings remain separate from clock offsets and V/F curves:

```yaml
# Inside the selected GPU's configuration; values here only illustrate units.
nvidia_rail_controls:
  nvvdd:
    min_voltage_uv: 800000
    max_voltage_uv: 850000
    overvoltage_uv: 1100000
    ocp_limit_ma: 88000
  msvdd: {}
```

The GUI displays millivolts and amperes, accepting up to three fractional digits;
the configuration and protocol preserve integer microvolts and milliamperes.
For example, `850.125 mV` is `850125 uV`, and `88.2 A` is `88200 mA`. Voltage
controls do not invent slider endpoints when the provider has no verified range.
OCP uses its reported configuration bounds, which are not an electrical safety
recommendation.

| Setting | Meaning |
| --- | --- |
| Minimum (VMIN) | Requested minimum-voltage policy |
| Maximum reliability (REL) | Requested reliability-voltage ceiling |
| Overvoltage (OV) | Separate overvoltage-policy ceiling, in advanced controls |
| OCP | Configured output-current protection threshold, in advanced controls |

Absolute voltage entries are estimates at application time. The provider converts
them to policy offsets and retains those concrete offsets when the same request
is reapplied, avoiding cumulative drift from rounding or temperature. Requested
values and fresh evaluated readings are shown separately. Other active limits
and firmware voltage steps still apply: equal minimum and maximum requests do
not establish a fixed physical voltage. OCP readback is a configured threshold,
not measured current. These runtime controls do not edit VBIOS images or VFE
equations. `Maximum (REL)` adjusts the reliability policy only; an operating
limit or OV can still constrain the separately displayed effective maximum.
The rail reference is a driver-reported value: older providers send it as
`reported_voltage_uv`, newer ones as `telemetry.default_uv`, and both read the
same field. It is not a measured rail voltage or a verified physical ADC
measurement. `Refresh readings` updates these readings without changing pending
entries or reloading the GPU configuration.

Optional telemetry is read-only and is not saved as a requested setting. The
provider can report the following values independently of whether a policy is
writable:

| Reading | Meaning |
| --- | --- |
| Target voltage | Driver-reported voltage target |
| Reference/default voltage | Driver-reported reference; not the provider's ownership restoration baseline |
| Sensed voltage | Driver-reported sensed voltage, separate from requested or evaluated policies |
| Current | Reported rail output current, separate from the configured OCP threshold |
| Arbitrated OCP | Reported threshold after policy arbitration, separate from the configured OCP request |
| ADC voltage | A sample from an identified ADC sensor; rail association is shown only when the provider has established it |

Unmapped ADC sensors retain their sensor identities and an unknown rail mapping;
LACT does not infer NVVDD or MSVDD from a similar voltage. Missing measurements
are omitted, and reported unavailability reasons are shown in the telemetry
details. These readings describe the last successful query, not continuous
sampling or independently verified physical measurements.

A provider can also report each rail's alternate reliability limit (ALT/VOP) as
a read-only reading. Firmware evaluates it separately from `Maximum (REL)` and
from OV; how it combines with them is not documented, and its value is neither a
fixed voltage nor evidence that the limit is being applied. LACT shows its
runtime selector and fresh evaluation, the selector alone when no evaluation is
available, or that the rail has no runtime selector. ALT/VOP has no requested
value, LACT cannot change it, and its runtime selector does not identify a VBIOS
policy row. No ALT/VOP line means the provider could not establish the rail's
identity or predates this reading.

Leave an entry blank to release only that field and restore its value from before
LACT took ownership. `Restore all rail controls` (or removing
`nvidia_rail_controls`) releases all fields owned by the provider through the
normal Apply flow, including when managed recovery is reported unavailable.
An empty object requests no new values; releasing previously owned fields can
still require restoration writes. Restoring ownership differs from forcing zeros
or factory values; preexisting rail settings from outside LACT are preserved.

NVIDIA Apply is incremental by settings group. A power-cap-only change does
not reset or reapply clocks, VRAM settings, curves, propagation ratios or fans;
a rail-only change likewise preserves those groups. Power cap, PowerMizer, fan
and target-temperature settings are applied first: they do not depend on rail
policies, so a later rail or clock failure cannot leave them unapplied. Clock
changes still perform the necessary clock reset, then prepare voltage boost and
rail policies before applying the new clock settings. Startup, controller reload
and explicit resets invalidate the relevant cached state. Only a fully
successful Apply becomes the new baseline, so a partial failure cannot turn
restoration of the previous configuration into a skipped operation. Releasing
rail controls still checks the provider's durable ownership even when the local
configuration is unchanged. Voltage boost and target-temperature failures are
logged as warnings instead of failing the Apply, and the next Apply retries them.
This includes restoring the default target temperature after removing a
configured one: the target read at startup, or just before LACT first changed
it if it was unreadable then.

If the provider cannot run, Apply still sets the power cap, PowerMizer mode,
fans and target temperature, and keeps voltage boost, rail and clock settings
pending. This covers a missing or unusable executable or Python environment, a
failed start, invalid output, and a timeout after which the provider process was
confirmed to have exited. Startup, configuration reloads and profile switches
report an error but keep those independent settings applied. An Apply from the
GUI that changes only independent settings keeps them under the normal
confirmation and revert while earlier voltage settings keep waiting; one that
changes voltage boost, rails or clocks is rejected, and the previous settings
are applied the same way. For a GPU without saved settings, the defaults are the
previous settings. Once the provider works again, Apply again or restart the
daemon to apply the pending settings.

Every setting stays blocked instead, until a provider reply shows no pending
recovery, when the provider reported pending recovery, when this daemon has an
unfinished change, when a provider call that could have written ended without a
valid reply (even if the request that started it was cancelled), when the
provider refused a request with its own error (for example because another
provider process holds the GPU's lock), or while a killed provider process is
not confirmed to have exited, even if the executable is missing by then. GPU
reloads keep this state; a daemon restart starts from the provider's next reply.
A new power limit can change policy state that an unfinished native provider
transaction verifies, which may then require manual recovery. Power cap,
PowerMizer, fan and target-temperature settings are each attempted even if
another one of them fails.

Cleaning up an NVIDIA GPU's controller on shutdown, a profile switch, a GPU
reload or a detach tries to return each of its fans to automatic control and
logs any failure. If a reload or profile switch cannot recover the provider or
reset clocks first, LACT skips the rest of the cleanup, which would release rail
policies, but still stops the fan control loop and tries to hand the fans back.
The next Apply writes the fan settings again.

Providers advertising `capabilities.profile_transactions: 1` support a durable
profile transaction. LACT stages rail policies with a checkpoint before applying
the subsequent clock, fan and thermal changes, and commits after all groups
succeed. On failure, it compensates any newly attempted frequency settings while
preserving voltage boost and domain voltage demands, then rolls back the exact
rail values and ownership checkpoint. An unconfirmed compensation or rollback
retains recovery state and blocks further changes. Returning frequency settings
to their software baseline does not guarantee lower physical frequency or
stability, especially when previous offsets were negative.

Older protocol-1 providers remain compatible through an apply-last fallback:
LACT defers their rail Apply until the other groups succeed. This fallback does
not provide a checkpoint spanning the whole profile change or guarantee that
rail policies take effect before new clock settings. Updating the provider is
required for those transaction guarantees.

Apply uses LACT's normal confirmation and revert flow. Configuration mutations
are serialized through application and confirmation/reversion. Competing edits
and profile changes are rejected while a confirmation is pending. Reset,
shutdown and configuration reload wait for the active operation. An
administrator's file edit deferred by confirmation is saved again when resumed,
so the older confirmation cannot leave the file and active configuration at
different revisions.

The provider saves recovery information before native writes, validates identity
and readback, and restores pre-transaction values on a failed change. Unconfirmed
recovery remains explicit and blocks new settings. The GUI keeps the recovery
reason visible and offers `Restore all rail controls` even when new writes are
unavailable or no requested fields remain in the local configuration. A known
interrupted transaction is recovered before new settings or resets; an unknown
checkpoint requires an explicit release request. Recovery can still fail, in
which case its record is retained. Unexpected changes outside owned controls
require manual inspection rather than silently discarding the recovery record.
A new boot starts from new readings; it does not replay old opaque buffers or old
ownership baselines.

The GUI and protocol do not extend XBAR/SYS/Video V/F-curve editing. Their existing
clock/domain settings retain their own controls and configuration.
