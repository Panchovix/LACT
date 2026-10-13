language = Language
language-system-default = System Default
language-restart-notice = Restart the LACT GUI to apply language changes.

experimental-features = Experimental Features
nvidia-pstate-offsets = NVIDIA per-P-state offsets
nvidia-pstate-offsets-description = Expose offsets for non-P0 states. Disabled by default because no known device/driver version supports it.

info-page = Hardware Info
oc-page = Overclocking
thermals-page = Thermals
software-page = Software Info
displays-page = Display Info
detach-page = Detach {$page}
page-detached = This page is open in another window.
show-page-window = Focus Window
reattach-page = Return to Main Window

hardware-info = Hardware Information

system-section = System
lact-daemon = LACT Daemon
lact-gui = LACT GUI
kernel-version = Kernel Version

instance = Instance
device-name = Device Name
platform-name = Platform Name
api-version = API Version
version = Version
driver-name = Driver Name
driver-version = Driver Version
compute-units = Compute Units
cl-c-version = OpenCL C Version
workgroup-size = Workgroup Size
global-memory = Global Memory
local-memory = Local Memory
features = Features
extensions = Extensions
show-button = Show
device-not-found = {$kind} device not found
cache-info = Cache Information
hw-ip-info = Hardware IP Information
hw-queues = Queues
amd-cache-desc = {$size} L{$level} {$types} cache { $shared ->
    [1] local to each CU
    *[other] shared between {$shared} CUs
}
nvidia-cache-desc = {$size} L{$level}
cache-data = Data
cache-instruction = Data
cache-cpu = CPU

monitoring-section = Monitoring
thresholds-section = Thresholds &amp; Limits
fan-control-section = Fan Control
temperatures = Temperatures
oc-missing-fan-control-warning = Warning: Overclocking support is disabled, fan control functionality is not available.
fan-speed = Fan Speed
no-fan-detected = No fan detected
no-sensors-found = No sensors found
throttling = Throttling
auto-page = Automatic
curve-page = Curve
static-page = Static
target-temp = Target temperature (°C)
acoustic-limit = Acoustic Limit (RPM)
acoustic-target = Acoustic Target (RPM)
min-fan-speed = Minimum Fan Speed (%)
zero-rpm = Zero RPM
zero-rpm-stop-temp = Zero RPM stop temperature (°C)
static-speed = Static Speed (%)
reset-button = Reset
reset-now-button = Reset Now
default-button = Default
pmfw-reset-warning = Warning: this resets the fan firmware settings!

temperature-sensor = Temperature Sensor
spindown-delay = Spindown Delay (ms)
spindown-delay-tooltip = How long the GPU needs to remain at a lower temperature value before ramping down the fan
speed-change-threshold = Speed Change Threshold (°C)
automatic-mode-threshold = Automatic Mode Threshold (°C)
automatic-mode-threshold-tooltip = Switch fan control to auto mode when the temperature is below this point.

    Many Nvidia GPUs only support stopping the fan in the automatic fan control mode, while a custom curve has a limited speed range such as 30-100%.

    This option allows to work around this limitation by only using the custom curve when above a specific temperature, with the card's builtin auto mode that supports zero RPM being used below it.

amd-oc = AMD Overclocking
amd-oc-disabled = AMD Overclocking is not enabled! <a href="https://github.com/ilya-zlobintsev/LACT/wiki/Overclocking-(AMD)">Some functionality will not be available.</a>
amd-oc-status = AMD Overclocking is currently: <b>{$status ->
    [true] Enabled
    [false] Disabled
    *[other] Unknown
}</b>
amd-oc-detected-system-config = Detected system configuration: <b>{$config ->
    [unsupported] Unsupported
    *[other] {$config}
}</b>
amd-oc-description =
    {$config ->
        [rpm-ostree] This option will toggle AMD overdrive support by setting boot flags through <b>rpm-ostree</b>.
        [unsupported]
            The current system is not recognized as supported for automatic overdrive configuration.
            You may attempt to enable overclocking from LACT, but a manual initramfs regeneration may be required for it to take effect.
            If that fails, a fallback option is to add <b>amdgpu.ppfeaturemask=0xffffffff</b> as a boot parameter in your bootloader.
        *[other] This option will toggle AMD overdrive support by creating a file at <b>{$path}</b> and updating the initramfs.
    }

    See <a href="https://github.com/ilya-zlobintsev/LACT/wiki/Overclocking-(AMD)">the wiki</a> for more information.
enable-amd-oc-description = This will enable the overdrive feature of the amdgpu driver by creating a file at <b>{$path}</b> and updating the initramfs. Are you sure you want to do this?
disable-amd-oc = Disable AMD Overclocking
enable-amd-oc = Enable AMD Overclocking
disable-amd-oc-description = This will disable AMD overclocking support (overdrive) on next reboot.
amd-oc-updating-configuration = Updating configuration (this may take a while)
amd-oc-updating-done = Configuration updated, please reboot to apply changes.

reset-config = Reset Configuration
reset-config-description = This will reset all GPU settings to their defaults and permanently delete all profiles

apply-button = Apply
confirm = Confirm
confirm-settings = Confirm Settings
revert-button = Revert
settings-confirmation = Do you want to keep the new settings? (Reverting in {$seconds_left} seconds)

power-cap = Power Limit
nvidia-power-cap-ioctl = Use experimental NVIDIA power control
nvidia-power-cap-ioctl-warning = Warning: uses an undocumented driver interface for all power limits, including resets. This allows values below the VBIOS minimum and may cause instability or stop working after driver updates. Enable at your own risk.
power-section = Power

watt = W
ghz = GHz
mhz = MHz
mv = mV
bytes = bytes
kibibyte = KiB
mebibyte = MiB
gibibyte = GiB

stats-section = Statistics
gpu-clock = GPU Core Clock
core-section = Core
gpu-clock-avg = GPU Core Clock (Average)
gpu-clock-target = GPU Core Clock (Target)
gpu-voltage = GPU Voltage
gpu-temp = Temperature
gpu-usage = GPU Usage
vram-clock = VRAM Clock
vram-section = VRAM
power-usage = Power Usage
extra-clocks = Extra Clocks
no-throttling = No
unknown-throttling = Unknown
missing-stat = N/A
vram-usage = VRAM Usage:
gtt-usage = GTT Usage:

igpu-section = Integrated Graphics
uma-carveout = UMA Carveout Size
uma-carveout-caption = Amount of memory reserved as dedicated VRAM. Requires a reboot to take effect.

performance-level-auto = Automatic
performance-level-high = Highest Clocks
performance-level-low = Lowest Clocks
performance-level-manual = Manual
performance-level-profile-standard = Profiling Standard
performance-level-profile-min-sclk = Profiling Lowest GPU Clock
performance-level-profile-min-mclk = Profiling Lowest VRAM Clock
performance-level-profile-peak = Profiling Peak
performance-level-auto-description = Automatically adjust GPU and VRAM clocks. (Default)
performance-level-high-description = Always use the highest clockspeeds for GPU and VRAM
performance-level-low-description = Always use the lowest clockspeeds for GPU and VRAM
performance-level-manual-description = Manual performance control
performance-level-profile-standard-description = Fixed profiling mode
performance-level-profile-min-sclk-description = Forces the GPU clock to lowest level
performance-level-profile-min-mclk-description = Forces the VRAM clock to lowest level
performance-level-profile-peak-description = Forces GPU and VRAM clocks to highest levels

performance-level = Performance Level
power-profile-mode = Power Profile Mode
manual-level-needed = Performance level has to be set to "manual" to use power states and modes
power-mizer-mode = PowerMizer Mode
power-mizer-mode-auto = Auto
power-mizer-mode-adaptive = Adaptive
power-mizer-mode-prefer-maximum-performance = Prefer Maximum Performance
power-mizer-mode-prefer-consistent-performance = Prefer Consistent Performance
power-mizer-mode-auto-description = Let the driver choose the performance policy.
power-mizer-mode-adaptive-description = Adjust GPU clocks based on GPU usage.
power-mizer-mode-prefer-maximum-performance-description = Favor maximum performance within driver limits.
power-mizer-mode-prefer-consistent-performance-description = Lock to GPU base clocks.

overclock-section = Clockspeed and Voltage
advanced-features = Advanced features
show-all-pstates = All P-States
enable-locked-clocks = Locked Clocks
enable-vf-curve = Custom VF Curve
pstate-list-description = <b>The following values are clock offsets for each P-State, going from highest to lowest.</b>
no-clocks-data = No clocks data available
reset-oc-tooltip = Warning: this resets all clock settings to defaults!
vf-curve-editor = VF Curve Editor
nvidia-vf-curve-warning = The voltage-frequency curve editor relies on undocumented driver functionality.
    <span weight = "heavy" underline = "single">Use at your own risk</span>.
voltage = Voltage
frequency = Frequency
vf-active-curve = Active Curve
vf-base-curve = Base Curve
vf-curve-visible-range = Visible Range (%):
vf-curve-visible-range-to = to
vf-curve-flatten-right = Flatten curve to the right
vf-curve-flatten-selection = Flatten selection
vf-curve-editing-disabled = VF curve editing is disabled on the OC page

gpu-clock-offset = Clock Offset
max-clock = Maximum Clock
max-gpu-voltage = Maximum Voltage
min-clock = Minimum Clock
min-gpu-voltage = Minimum Voltage
gpu-voltage-offset = Voltage Offset
gpu-voltage-boost = Voltage Boost
gpu-voltage-boost-tooltip = Controls how much of the additional voltage headroom defined by the driver is available. 100% means all of this headroom, not 100% of total GPU voltage. More headroom may sustain higher clockspeeds but increases power draw and heat.
pstate-clock-offset = P-State {$pstate} Clock Offset
pstate-clock = P-State {$pstate} Clock
pstate-clock-voltage = P-State {$pstate} Voltage
clock-domain-offset = {$domain} Clock Offset
clock-domain-voltage-offset = {$domain} MSVDD Offset
msvdd-offset = MSVDD Offset
msvdd-offset-mixed = MSVDD Offset — Mixed
msvdd-offset-tooltip = Writes the same offset into each of the clock domains below. The driver has no single control for the MSVDD rail, so this is a convenience that sets all of them at once, not a rail voltage setting of its own; changing an individual domain afterwards overrides it for that domain.
    Each offset shifts how much voltage its own domain asks for at a given clockspeed, and the domains share one rail. A positive offset therefore lowers that domain's clocks, while a negative one raises them slightly at the cost of the margin that keeps memory traffic correct.
advanced-section = Advanced
advanced-section-description = Clock domains the driver does not expose through its normal interface. Independent of the locked clock options above, so both can be used together.
xbar-ratio = GPC Clock Propagation Ratio
xbar-ratio-tooltip = Ratio at which the GPC clock propagates to XBAR, 90% by default. Raising it decouples XBAR from the core clock without changing the core clock itself.

    SYS, Power and Legacy follow XBAR, so they rise with it. GPC, memory, HUB and Display are unaffected.

    This raises the voltage the card requests to sustain the higher clocks, so it interacts with the power limit.

pstate = Power State
pstates = Power States
gpu-pstates = GPU Power States
vram-pstates = VRAM Power States
pstates-manual-needed = To enable P-State configuration, the performance level must be set to Manual.
enable-pstate-config = P-State Configuration

menu = Menu
show-historical-charts = Show Graphs
show-process-monitor = Show Process Monitor
generate-debug-snapshot = Generate Debug Snapshot
dump-vbios = Dump VBIOS
reset-all-config = Reset All Configuration
stats-update-interval = Update Interval (ms)

historical-data-title = Historical Data
graphs-per-row = Graphs Per Row:
time-period-seconds = Time Period (Seconds):
reset-all-graphs-tooltip = Reset All Graphs To Default
add-graph = Add Graph
delete-graph = Delete Graph
edit-graphs = Edit
export-csv = Export as CSV
edit-graph-sensors = Edit Graph Sensors

error-heading = Error
daemon-info-heading = Daemon info

reconnecting-to-daemon = Service connection lost, reconnecting...
daemon-connection-lost = Connection Lost
service-explanation =
    Applying GPU settings requires the LACT system service.
    Without it, LACT runs in standalone mode, where only information and monitoring are available.
service-setup-title = Service Setup
setup-error = Setup error: {$error}
service-connection-status = Connection Status
service-status = Service Status
service-permission-denied =
    Permission denied, service is not configured to allow connections from your user.
    See <a href="https://github.com/ilya-zlobintsev/lact#configuration">GitHub</a> for more information
service-connected = connected
service-disconnected = not connected
service-version = Service Version
gui-version = GUI Version
service-version-mismatch = mismatched
service-logs = Service Logs

service-start = Start
service-stop = Stop
service-restart = Restart
service-autostart = Autostart on boot
service-autostart-disable = Also disable autostart

version-mismatch-description =
    Version mismatch between GUI and Daemon ({$gui_version}-{$gui_commit} vs {$daemon_version}-{$daemon_commit})!
    If you have updated LACT, you need to restart the service.

plot-show-detailed-info = Show detailed info

display-title = Display {$identifier}
display-manufacturer = Manufacturer
display-product-code = Product Code
display-model = Model
display-physical-size = Physical Size
display-connection = Connection
display-manufacture-date = Manufacture Date
displays-missing = No Displays Detected

settings-profile = Settings Profile
auto-switch-profiles = Switch automatically
add-profile = Add new profile
import-profile = Import profile from file

create-profile = Create Profile
name = Name
profile-copy-from = Copy settings from:
create = Create
cancel = Cancel
close = Close
save = Save
default-profile = Default
rename-profile = Rename Profile
rename-profile-from = Rename profile <b>{$old_name}</b> to:
delete-profile = Delete Profile
edit-rules = Edit Rules
edit-rule = Edit Rule
remove-rule = Remove Rule
profile-rules = Profile Rules
export-to-file = Export To File
move-up = Move Up
move-down = Move Down
profile-activation = Activation
profile-hooks = Hooks
profile-activation-desc = Activate profile '{$name}' when:
any-rules-matched = Any of the following rules are matched:
all-rules-matched = All of the following rules are matched:
activation-settings-status = Selected activation settings are currently <b>{ $matched ->
    [true] matched
    *[false] not matched
}</b>
activation-auto-switching-disabled = Automatic profile switching is currently disabled
profile-hook-command = Run a command when the profile '{$cmd}' is:
profile-hook-activated = Activated:
profile-hook-deactivated = Deactivated:
profile-hook-note = Note: these commands are executed as root by the LACT daemon, and do not have access to the desktop environment. As such, they cannot be used directly to launch graphical applications.

profile-rule-process-tab = A process is running
profile-rule-gamemode-tab = Gamemode is active
profile-rule-process-name = Process Name:
profile-rule-args-contain = Arguments Contain:
profile-rule-specific-process = With a specific process:

theme = Theme
theme-auto = Automatic
preferences = Preferences
ui = UI
daemon = Daemon
about = About

color-scheme = Color Scheme
color-scheme-auto = System
color-scheme-light = Light
color-scheme-dark = Dark

# Crash page
crash-page-title = Application Crashed
exit = Exit
msvdd-voltage = MSVDD Voltage
msvdd-curve-editor = MSVDD VF Curve
msvdd-curve-description = Voltage-frequency curves of the clock domains fed by the MSVDD rail. Shown for reference only: the driver reports no way to tell whether a change to one of these was adopted.

nvidia-rails-section = Voltages
nvidia-rails-enable = Enable overvoltage and current limit controls
nvidia-rails-advanced = Advanced: overvoltage and current limit
nvidia-rails-minimum = Requested minimum (VMIN)
nvidia-rails-maximum = Requested maximum reliability (REL)
nvidia-rails-overvoltage = Requested overvoltage limit (OV)
nvidia-rails-ocp = Requested current limit (OCP)
nvidia-rails-original = Original
nvidia-rails-policy-note = Blank fields restore their original values. Equal minimum and maximum requests do not lock physical voltage. Maximum changes REL; other voltage policies can still limit the effective maximum.
nvidia-rails-available = External runtime helper available. Enter requested policies; evaluated readback is shown separately.
nvidia-rails-unavailable = Rail control unavailable. Configure the external runtime helper in the daemon.
nvidia-rails-voltage-readback = Evaluated: { $evaluated } mV
nvidia-rails-current-readback = Reported OCP limit: { $current } A; range: { $minimum }–{ $maximum } A
nvidia-rails-effective = Effective maximum (last read): { $value } mV
nvidia-rails-alt-vop = Alternate reliability limit (ALT/VOP, read only, last read): { $value } mV, runtime selector { $selector }
nvidia-rails-alt-vop-unknown = Alternate reliability limit (ALT/VOP, read only): runtime selector { $selector }, no evaluation
nvidia-rails-alt-vop-absent = Alternate reliability limit (ALT/VOP): no runtime selector on this rail
nvidia-rails-invalid-value = Rail values must be positive decimal numbers with up to three fractional digits, or blank to restore the original value.
nvidia-rails-protections-section = NVIDIA protections
nvidia-rails-protections-note = Disabling restores only the managed OV and OCP fields. Voltage minimum and maximum requests remain active. These policy requests do not guarantee electrical limits.
nvidia-rails-refresh = Refresh readings
nvidia-rails-restore-all = Restore all rail controls

nvidia-rails-sensors = Sensor details (last read)
nvidia-rails-unknown-sensor-rail = rail not identified
nvidia-rails-adc-sample = ADC { $sensor } ({ $rail }): { $value } mV
nvidia-rails-telemetry-unavailable = { $feature }: { $reason }
nvidia-rails-target = Rail target (driver): { $value } mV
nvidia-rails-reference = Rail reference (driver): { $value } mV
nvidia-rails-sensed = Sensed voltage (driver): { $value } mV
nvidia-rails-adc = ADC voltage: { $value } mV
nvidia-rails-current = Rail current reading: { $value } A
nvidia-rails-arbitrated-ocp = Effective OCP limit: { $value } A

nvidia-rails-pending = Saved rail requests remain pending; the rail bundle was not applied. Clocks, VRAM, power and fan settings can still be applied. Last deferred reason: { $reason }
