//! Prefer low-latency BLE connection parameters (Windows 11+).
//!
//! btleplug's `request_connection_parameters` drops the WinRT request handle
//! immediately, which can revert the preference. We keep our own request alive
//! for the whole session instead.

/// Held preference request. Drop to release (may restore OS defaults).
pub struct LowLatencyHold {
    #[cfg(windows)]
    _inner: Option<windows_hold::Inner>,
}

impl LowLatencyHold {
    /// No-op placeholder when preference could not be applied.
    pub fn none() -> Self {
        Self {
            #[cfg(windows)]
            _inner: None,
        }
    }

    /// Ask Windows for ThroughputOptimized and keep the request alive.
    ///
    /// `addr_u64` must match btleplug's `BDAddr` → `u64` layout (big-endian MSB).
    pub async fn acquire(addr_u64: u64) -> (Self, Option<f32>) {
        #[cfg(windows)]
        {
            match windows_hold::acquire(addr_u64).await {
                Ok((inner, interval_ms)) => (
                    Self {
                        _inner: Some(inner),
                    },
                    Some(interval_ms),
                ),
                Err(_) => (Self::none(), None),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = addr_u64;
            (Self::none(), None)
        }
    }
}

#[cfg(windows)]
mod windows_hold {
    use windows::Devices::Bluetooth::{
        BluetoothLEDevice, BluetoothLEPreferredConnectionParameters,
        BluetoothLEPreferredConnectionParametersRequest,
    };

    pub struct Inner {
        _device: BluetoothLEDevice,
        // Dropping this can restore balanced/power defaults — keep for session.
        _request: BluetoothLEPreferredConnectionParametersRequest,
    }

    pub async fn acquire(addr_u64: u64) -> anyhow::Result<(Inner, f32)> {
        let device = BluetoothLEDevice::FromBluetoothAddressAsync(addr_u64)?
            .await
            .map_err(|e| anyhow::anyhow!("FromBluetoothAddressAsync: {e}"))?;

        let params = BluetoothLEPreferredConnectionParameters::ThroughputOptimized()?;
        let request = device.RequestPreferredConnectionParameters(&params)?;
        // Success = 1 (BluetoothLEPreferredConnectionParametersRequestStatus)
        let status = request.Status()?.0;
        if status != 1 {
            anyhow::bail!("RequestPreferredConnectionParameters status={status}");
        }

        let interval_ms = match device.GetConnectionParameters() {
            Ok(cp) => cp.ConnectionInterval().unwrap_or(0) as f32 * 1.25,
            Err(_) => 0.0,
        };

        Ok((
            Inner {
                _device: device,
                _request: request,
            },
            interval_ms,
        ))
    }
}
