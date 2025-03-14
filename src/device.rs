//! This module contains [SnpDevice] and helpers for it.

use core::cmp::min;

use log::error;
use smoltcp::{
    phy::{Device, DeviceCapabilities, Medium},
    wire::EthernetAddress,
};
use uefi::{
    Status,
    proto::network::snp::{ReceiveFlags, SimpleNetwork},
};

use crate::convert::u2s_mac_address;

/// The default `MAX_PACKET` parameter for [SnpDevice]s.
pub const DEFAULT_MAX_PACKET: usize = 1500;

/// A smoltcp [Device] based on a uefi [SimpleNetwork].
///
/// This device assumes that your [SimpleNetwork] has already been initialized.
/// Here's an example of how to do that:
///
/// ```no_run
/// # fn main() -> Result<(), uefi::Error> {
/// let handle = boot::get_handle_for_protocol::<SimpleNetwork>()?;
/// let snp = boot::open_protocol_exclusive::<SimpleNetwork>(handle)?;
/// snp.start()?;
/// snp.initialize(0, 0);
///
/// let mut device = SnpDevice::new(snp)?;
/// # }
/// ```
///
/// # Implementation notes
///
/// Note that currently, this does no allocations whatsoever. On every transmit
/// and receive, a new buffer is created on-stack. This is obviously very inefficient,
/// but this is also very temporary.
///
/// To adjust how big this on-stack buffer is, you can set the `MAX_PACKET` parameter,
/// which is by default set to [`DEFAULT_MAX_PACKET`]. Note that if you set this parameter
/// lower than the actual packet size of the [SimpleNetwork], advertised capabilities
/// in [`Device::capabilities`] will be adjusted to match.
pub struct SnpDevice<'a, const MAX_PACKET: usize = DEFAULT_MAX_PACKET> {
    snp: &'a SimpleNetwork,
}

impl<'a, const MAX_PACKET: usize> SnpDevice<'a, MAX_PACKET> {
    /// Create an [SnpDevice] based on the provided [SimpleNetwork].
    ///
    /// Note that this will set receive filters on the [SimpleNetwork] as well.
    /// This may error if that fails.
    pub fn new(snp: &'a SimpleNetwork) -> Result<Self, uefi::Error> {
        snp.receive_filters(
            ReceiveFlags::UNICAST | ReceiveFlags::MULTICAST | ReceiveFlags::BROADCAST,
            ReceiveFlags::empty(),
            false,
            None,
        )?;

        Ok(Self { snp })
    }

    /// Get the current MAC address configured on the underlying [SimpleNetwork].
    pub fn current_address(&self) -> EthernetAddress {
        u2s_mac_address(&self.snp.mode().current_address)
    }

    /// Get the permanent MAC address configured on the underlying [SimpleNetwork].
    pub fn permanent_address(&self) -> EthernetAddress {
        u2s_mac_address(&self.snp.mode().permanent_address)
    }

    /// Get a reference to the underlying [SimpleNetwork].
    pub fn snp(&self) -> &SimpleNetwork {
        self.snp
    }
}

impl<'a, const MAX_PACKET: usize> Device for SnpDevice<'a, MAX_PACKET> {
    type RxToken<'b>
        = SnpRxToken<MAX_PACKET>
    where
        Self: 'b;

    type TxToken<'b>
        = SnpTxToken<'b, MAX_PACKET>
    where
        Self: 'b;

    fn receive(
        &mut self,
        _timestamp: smoltcp::time::Instant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let mut rx = SnpRxToken {
            packet: [0; MAX_PACKET],
            size: 0,
        };

        match self.snp.receive(&mut rx.packet, None, None, None, None) {
            Ok(size) => {
                rx.size = size;
                Some((rx, SnpTxToken { snp: self.snp }))
            }
            Err(e) if e.status() == Status::NOT_READY => {
                // NOT_READY indicates no packets received yet.
                None
            }
            Err(e) => {
                error!("error during rx: {e}");
                None
            }
        }
    }

    fn transmit(&mut self, _timestamp: smoltcp::time::Instant) -> Option<Self::TxToken<'_>> {
        Some(SnpTxToken { snp: self.snp })
    }

    fn capabilities(&self) -> smoltcp::phy::DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = min(self.snp.mode().max_packet_size as usize, MAX_PACKET);
        caps
    }
}

pub struct SnpRxToken<const MAX_PACKET: usize> {
    packet: [u8; MAX_PACKET],
    size: usize,
}

impl<const MAX_PACKET: usize> smoltcp::phy::RxToken for SnpRxToken<MAX_PACKET> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.packet[..self.size])
    }
}

pub struct SnpTxToken<'a, const MAX_PACKET: usize> {
    snp: &'a SimpleNetwork,
}

impl<'a, const MAX_PACKET: usize> smoltcp::phy::TxToken for SnpTxToken<'a, MAX_PACKET> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = [0u8; MAX_PACKET];
        let packet = &mut buf[..len];
        let result = f(packet);

        self.snp
            .transmit(0, packet, None, None, None)
            .inspect_err(|e| {
                error!("error during tx: {e}");
            })
            .ok();

        result
    }
}
