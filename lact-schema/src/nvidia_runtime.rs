//! Typed information and settings for the optional external NVIDIA runtime helper.
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaRailControls {
    #[serde(default)]
    pub nvvdd: NvidiaRailSettings,
    #[serde(default)]
    pub msvdd: NvidiaRailSettings,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaRailSettings {
    pub min_voltage_uv: Option<u32>,
    pub max_voltage_uv: Option<u32>,
    pub overvoltage_uv: Option<u32>,
    pub ocp_limit_ma: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaRuntimeDevice {
    pub uuid: String,
    pub bdf: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaRuntimeInfo {
    pub protocol_version: u32,
    pub available: bool,
    pub reason: Option<String>,
    pub device: NvidiaRuntimeDevice,
    pub managed: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub recovery_required: bool,
    pub rails: Vec<NvidiaRuntimeRail>,
    #[serde(default, skip_serializing_if = "NvidiaRuntimeCapabilities::is_empty")]
    pub capabilities: NvidiaRuntimeCapabilities,
    pub transaction_id: Option<String>,
    pub transaction_status: Option<String>,
    pub request_id: Option<String>,
    pub telemetry: Option<NvidiaRuntimeTelemetry>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaRuntimeRail {
    pub rail: NvidiaRail,
    pub min_voltage: Option<NvidiaVoltagePolicy>,
    pub max_voltage: Option<NvidiaVoltagePolicy>,
    pub overvoltage: Option<NvidiaVoltagePolicy>,
    pub ocp_limit: Option<NvidiaOcpLimit>,
    pub effective_max_uv: Option<u32>,
    pub reported_voltage_uv: Option<u32>,
    pub telemetry: Option<NvidiaRailTelemetry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaRuntimeCapabilities {
    #[serde(default)]
    pub profile_transactions: u32,
}

impl NvidiaRuntimeCapabilities {
    pub fn is_empty(&self) -> bool {
        self.profile_transactions == 0
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

/// Optional measurements are independent of policy-control availability.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaRailTelemetry {
    pub target_uv: Option<u32>,
    pub default_uv: Option<u32>,
    pub sensed_uv: Option<u32>,
    pub adc_uv: Option<u32>,
    pub current_ma: Option<u32>,
    pub arbitrated_ocp_ma: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaRuntimeTelemetry {
    #[serde(default)]
    pub adc_samples: Vec<NvidiaAdcSample>,
    #[serde(default)]
    pub unavailable: Vec<NvidiaTelemetryUnavailable>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaAdcSample {
    pub sensor_id: u32,
    /// None means that the provider has not established this sensor's rail.
    pub rail: Option<NvidiaRail>,
    pub voltage_uv: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaTelemetryUnavailable {
    pub feature: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NvidiaRail {
    #[serde(rename = "NVVDD")]
    Nvvdd,
    #[serde(rename = "MSVDD")]
    Msvdd,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaVoltagePolicy {
    pub evaluated_uv: u32,
    pub offset_uv: i32,
    pub writable: bool,
    pub requested_uv: Option<u32>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NvidiaOcpLimit {
    pub current_ma: u32,
    pub minimum_ma: u32,
    pub maximum_ma: u32,
    pub writable: bool,
    pub requested_ma: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GpuConfig;

    #[test]
    fn missing_rails_and_fields_default_without_losing_opt_in() {
        let settings: NvidiaRailControls =
            serde_json::from_str(r#"{"nvvdd":{"min_voltage_uv":900125}}"#).unwrap();
        assert_eq!(settings.nvvdd.min_voltage_uv, Some(900125));
        assert_eq!(settings.msvdd, NvidiaRailSettings::default());
        assert_eq!(
            serde_json::to_value(settings).unwrap(),
            serde_json::json!({"nvvdd":{"min_voltage_uv":900125},"msvdd":{}})
        );
        let off: GpuConfig = serde_json::from_str("{}").unwrap();
        assert!(off.nvidia_rail_controls.is_none());
        assert!(
            serde_json::to_value(off)
                .unwrap()
                .get("nvidia_rail_controls")
                .is_none()
        );
        let on: GpuConfig = serde_json::from_str(r#"{"nvidia_rail_controls":{}}"#).unwrap();
        assert_eq!(on.nvidia_rail_controls, Some(NvidiaRailControls::default()));
        assert!(
            serde_json::to_value(on)
                .unwrap()
                .get("nvidia_rail_controls")
                .is_some()
        );
    }

    #[test]
    fn status_round_trips_exact_integer_units_and_rail_identity() {
        let raw = serde_json::json!({"protocol_version":1,"available":true,"device":{"uuid":"GPU-test","bdf":"0000:09:00.0"},"managed":false,"rails":[{"rail":"MSVDD","min_voltage":{"evaluated_uv":900125,"offset_uv":-125,"writable":true},"ocp_limit":{"current_ma":123456,"minimum_ma":1000,"maximum_ma":200000,"writable":false}}]});
        let info: NvidiaRuntimeInfo = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(info.rails[0].rail, NvidiaRail::Msvdd);
        assert_eq!(serde_json::to_value(info).unwrap(), raw);
    }
    #[test]
    fn optional_telemetry_preserves_units_and_unknown_sensor_identity() {
        let raw = serde_json::json!({"protocol_version":1,"available":true,
            "device":{"uuid":"GPU-test","bdf":"0000:09:00.0"},"managed":false,
            "capabilities":{"profile_transactions":1},
            "rails":[{"rail":"NVVDD","telemetry":{"target_uv":950125,"current_ma":123456,"arbitrated_ocp_ma":180000}}],
            "telemetry":{"adc_samples":[{"sensor_id":7,"rail":null,"voltage_uv":947875}],"unavailable":[]}});
        let info: NvidiaRuntimeInfo = serde_json::from_value(raw).unwrap();
        assert_eq!(info.capabilities.profile_transactions, 1);
        assert_eq!(
            info.rails[0].telemetry.as_ref().unwrap().current_ma,
            Some(123456)
        );
        assert_eq!(info.telemetry.as_ref().unwrap().adc_samples[0].rail, None);
        assert_eq!(
            info.telemetry.as_ref().unwrap().adc_samples[0].voltage_uv,
            947875
        );
        assert!(!info.recovery_required);
        assert!(info.transaction_id.is_none());
    }
}
