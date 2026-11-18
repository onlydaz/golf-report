//! Ethernet selection and read-only route checks shared by probe, reader and ACK transport.
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManualAdapter {
    pub id: String,
    pub machine: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Adapter {
    pub id: String,
    pub name: String,
    pub index: u32,
    pub luid: u64,
    pub ethernet: bool,
    pub up: bool,
    pub ipv4: Vec<Ipv4Addr>,
    pub prefixes: Vec<u8>,
}

const SELECT_AGAIN: &str = "Card Ethernet được chỉ định không còn phù hợp. Vui lòng liên hệ kỹ thuật để kiểm tra cấu hình triển khai.";
const ROUTE: &str =
    "Đường mạng đến PLC chưa đúng. Vui lòng liên hệ kỹ thuật để kiểm tra cấu hình mạng.";
const NO_ETHERNET: &str = "Chưa phát hiện kết nối Ethernet đến mạng PLC.";
const IPV4: &str = "Địa chỉ IPv4 của Ethernet chưa phù hợp với mạng PLC.";

pub(super) fn is_network_error(message: &str) -> bool {
    [
        NO_ETHERNET,
        IPV4,
        ROUTE,
        "Lỗi Modbus TCP AS200:",
        "Hết thời gian kết nối/đọc AS200",
    ]
    .iter()
    .any(|s| message.contains(s))
}

// Pure decision: the OS inventory and route are refreshed for every new socket.
fn choose(
    adapters: &[Adapter],
    target: Ipv4Addr,
    route: u32,
    source: Ipv4Addr,
    manual: Option<&ManualAdapter>,
    machine: &str,
    _legacy: Option<IpAddr>,
) -> Result<Option<IpAddr>, String> {
    let (adapter, preferred) = candidate(adapters, target, manual, machine)?;
    if adapter.index != route {
        return Err(ROUTE.into());
    }
    let source_matches = adapter
        .ipv4
        .iter()
        .zip(&adapter.prefixes)
        .any(|(ip, prefix)| *ip == source && same_subnet(*ip, *prefix, target));
    Ok(Some(if source_matches { source } else { preferred }.into()))
}

pub(crate) fn same_subnet(local: Ipv4Addr, prefix: u8, target: Ipv4Addr) -> bool {
    if prefix == 0 || prefix > 32 || local == target {
        return false;
    }
    let mask = u32::MAX << (32 - prefix);
    let host = u32::from(target) & !mask;
    (u32::from(local) & mask) == (u32::from(target) & mask)
        && (prefix >= 31 || (host != 0 && host != !mask))
}

pub(crate) fn candidate<'a>(
    adapters: &'a [Adapter],
    target: Ipv4Addr,
    manual: Option<&ManualAdapter>,
    machine: &str,
) -> Result<(&'a Adapter, Ipv4Addr), String> {
    if manual.is_some_and(|m| m.machine != machine) {
        return Err(SELECT_AGAIN.into());
    }
    let mut matches = Vec::new();
    for a in adapters
        .iter()
        .filter(|a| a.ethernet && a.up && manual.is_none_or(|m| m.id == a.id))
    {
        let mut ips: Vec<_> = a
            .ipv4
            .iter()
            .zip(&a.prefixes)
            .filter(|(ip, prefix)| same_subnet(**ip, **prefix, target))
            .collect();
        ips.sort_by_key(|(ip, prefix)| (std::cmp::Reverse(**prefix), **ip));
        if let Some((ip, _)) = ips.first() {
            matches.push((a, **ip));
        }
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(if adapters.iter().any(|a| a.ethernet && a.up && manual.is_none_or(|m|m.id==a.id)) { IPV4 } else { NO_ETHERNET }.into()),
        _ => Err("Nhiều Ethernet phù hợp với mạng PLC. Vui lòng liên hệ kỹ thuật để chỉ định card trong cấu hình triển khai.".into()),
    }
}

pub(super) fn pin_worker(
    config: &mut super::simulator_config::SimulatorConfig,
) -> Result<(), String> {
    config.plc_local_ip = None; // Compatibility only; do not write the TOML.
    let endpoint: SocketAddr = config
        .address
        .parse()
        .map_err(|_| "Địa chỉ PLC không hợp lệ")?;
    if endpoint.ip().is_loopback() {
        config.plc_adapter = None;
        return Ok(());
    }
    let IpAddr::V4(target) = endpoint.ip() else {
        return Err("PLC Ethernet cần IPv4".into());
    };
    let machine = platform::machine()?;
    let list = adapters()?;
    let manual = deployment_manual(&list, config.plc_adapter.as_ref(), &machine);
    let (a, _) = candidate(&list, target, manual, &machine)?;
    config.plc_adapter = Some(ManualAdapter {
        id: a.id.clone(),
        machine,
    });
    Ok(()) // Runtime only: reconnect/ACK cannot switch adapters until Worker is stopped.
}

// Apply only at the beginning of an explicit probe/new Engine. A running
// Engine's pin remains strict in resolve(), including when its adapter disappears.
pub(crate) fn deployment_manual<'a>(
    list: &[Adapter],
    manual: Option<&'a ManualAdapter>,
    machine: &str,
) -> Option<&'a ManualAdapter> {
    manual.filter(|m| m.machine == machine && list.iter().any(|a| a.id == m.id && a.ethernet))
}

pub fn adapters() -> Result<Vec<Adapter>, String> {
    platform::adapters()
}
pub fn selection(id: &str) -> Result<ManualAdapter, String> {
    if !adapters()?
        .iter()
        .any(|a| a.id == id && a.ethernet && a.up && !a.ipv4.is_empty())
    {
        return Err(SELECT_AGAIN.into());
    }
    Ok(ManualAdapter {
        id: id.into(),
        machine: platform::machine()?,
    })
}

pub(crate) fn resolve(
    endpoint: SocketAddr,
    legacy: Option<IpAddr>,
    manual: Option<&ManualAdapter>,
) -> Result<Option<IpAddr>, String> {
    if endpoint.ip().is_loopback() && manual.is_none() {
        return Ok(legacy);
    }
    let IpAddr::V4(ip) = endpoint.ip() else {
        return Err("Kết nối Ethernet PLC yêu cầu endpoint IPv4.".into());
    };
    let adapters = adapters()?;
    let machine = if manual.is_some() {
        platform::machine()?
    } else {
        String::new()
    };
    candidate(&adapters, ip, manual, &machine)?;
    let (route, source) = platform::route(ip).map_err(|_| ROUTE.to_string())?;
    choose(&adapters, ip, route, source, manual, &machine, legacy)
}

#[cfg(windows)]
pub(crate) mod platform {
    use super::*;
    use windows_sys::Win32::{
        NetworkManagement::IpHelper::*, Networking::WinSock::*, System::Registry::*,
    };

    pub fn machine() -> Result<String, String> {
        let key: Vec<u16> = "SOFTWARE\\Microsoft\\Cryptography\0"
            .encode_utf16()
            .collect();
        let name: Vec<u16> = "MachineGuid\0".encode_utf16().collect();
        let mut data = [0u16; 256];
        let mut size = std::mem::size_of_val(&data) as u32;
        // Read only; never persist or log this machine identifier outside the local config.
        let result = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
                std::ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if result != 0 {
            return Err(
                "Không xác minh được laptop cho cài đặt Ethernet. Chọn lại adapter trên máy này."
                    .into(),
            );
        }
        Ok(String::from_utf16_lossy(
            &data[..data.iter().position(|v| *v == 0).unwrap_or(data.len())],
        ))
    }

    pub fn adapters() -> Result<Vec<Adapter>, String> {
        // u64 storage guarantees alignment required by IP_ADAPTER_ADDRESSES.
        let mut size = 16384u32;
        for _ in 0..4 {
            let mut storage = vec![0u64; (size as usize).div_ceil(8)];
            let first = storage.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
            let result = unsafe {
                GetAdaptersAddresses(
                    AF_INET as u32,
                    GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER,
                    std::ptr::null(),
                    first,
                    &mut size,
                )
            };
            if result == 111 {
                continue;
            } // ERROR_BUFFER_OVERFLOW: inventory changed.
            if result == 232 {
                return Ok(Vec::new());
            } // ERROR_NO_DATA
            if result != 0 {
                return Err(format!(
                    "Không đọc được danh sách card mạng Windows (mã {result})."
                ));
            }
            let mut out = Vec::new();
            let mut ptr = first;
            // All pointers are owned by storage, which stays alive until traversal ends.
            unsafe {
                while !ptr.is_null() {
                    let a = &*ptr;
                    let mut len = 0;
                    if !a.FriendlyName.is_null() {
                        while *a.FriendlyName.add(len) != 0 {
                            len += 1;
                        }
                    }
                    let name = if len == 0 {
                        String::new()
                    } else {
                        String::from_utf16_lossy(std::slice::from_raw_parts(a.FriendlyName, len))
                    };
                    let id = std::ffi::CStr::from_ptr(a.AdapterName.cast())
                        .to_string_lossy()
                        .into_owned();
                    let mut ipv4 = Vec::new();
                    let mut prefixes = Vec::new();
                    let mut u = a.FirstUnicastAddress;
                    while !u.is_null() {
                        let entry = &*u;
                        if !entry.Address.lpSockaddr.is_null()
                            && (*entry.Address.lpSockaddr).sa_family == AF_INET
                            && entry.DadState == IpDadStatePreferred
                        {
                            let addr = &*entry.Address.lpSockaddr.cast::<SOCKADDR_IN>();
                            let ip = Ipv4Addr::from(addr.sin_addr.S_un.S_addr.to_ne_bytes());
                            if !ip.is_unspecified() {
                                ipv4.push(ip);
                                prefixes.push(entry.OnLinkPrefixLength);
                            }
                        }
                        u = entry.Next;
                    }
                    let mut info = MIB_IF_ROW2::default();
                    info.InterfaceLuid = a.Luid;
                    let physical = GetIfEntry2(&mut info) == 0
                        && info.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
                    out.push(Adapter {
                        id,
                        name,
                        index: a.Anonymous1.Anonymous.IfIndex,
                        luid: a.Luid.Value,
                        ethernet: physical && a.IfType == IF_TYPE_ETHERNET_CSMACD,
                        up: a.OperStatus == 1,
                        ipv4,
                        prefixes,
                    });
                    ptr = a.Next;
                }
            }
            return Ok(out);
        }
        Err("Danh sách card mạng đang thay đổi. Vui lòng thử lại.".into())
    }

    pub fn route(ip: Ipv4Addr) -> Result<(u32, Ipv4Addr), String> {
        let mut dest = SOCKADDR_INET::default();
        dest.Ipv4.sin_family = AF_INET;
        dest.Ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes(ip.octets());
        let mut row = MIB_IPFORWARD_ROW2::default();
        let mut source = SOCKADDR_INET::default();
        // No interface/source constraint: inspect the actual Windows preference.
        let result = unsafe {
            GetBestRoute2(
                std::ptr::null(),
                0,
                std::ptr::null(),
                &dest,
                0,
                &mut row,
                &mut source,
            )
        };
        if result != 0 {
            return Err(format!(
                "Windows chưa có tuyến tới PLC (mã {result}). Kiểm tra cáp Ethernet và subnet; không cần PLC khi làm việc ở nhà."
            ));
        }
        Ok((row.InterfaceIndex, unsafe {
            Ipv4Addr::from(source.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes())
        }))
    }
}

#[cfg(not(windows))]
pub(crate) mod platform {
    use super::*;
    pub fn adapters() -> Result<Vec<Adapter>, String> {
        Err("Chọn adapter PLC hiện chỉ hỗ trợ Windows.".into())
    }
    pub fn machine() -> Result<String, String> {
        Err("Chọn adapter PLC yêu cầu Windows.".into())
    }
    pub fn route(_: Ipv4Addr) -> Result<(u32, Ipv4Addr), String> {
        Err("Kiểm tra tuyến PLC yêu cầu Windows.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn nic(id: &str, index: u32, ethernet: bool, up: bool) -> Adapter {
        Adapter {
            id: id.into(),
            name: id.into(),
            index,
            luid: index as u64,
            ethernet,
            up,
            ipv4: vec![Ipv4Addr::new(192, 168, 10, 20)],
            prefixes: vec![24],
        }
    }
    #[test]
    fn automatic_manual_and_reconnect_resolve_current_ip() {
        let ip = Ipv4Addr::new(192, 168, 10, 20);
        let mut list = vec![nic("eth", 1, true, true)];
        let manual = ManualAdapter {
            id: "eth".into(),
            machine: "this".into(),
        };
        assert_eq!(
            choose(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                1,
                ip,
                None,
                "this",
                None
            )
            .unwrap(),
            Some(ip.into())
        );
        assert_eq!(
            choose(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                1,
                ip,
                Some(&manual),
                "this",
                None
            )
            .unwrap(),
            Some(ip.into())
        );
        let changed = Ipv4Addr::new(192, 168, 10, 21);
        list[0].ipv4 = vec![changed];
        assert_eq!(
            choose(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                1,
                changed,
                Some(&manual),
                "this",
                None
            )
            .unwrap(),
            Some(changed.into())
        );
        assert!(
            choose(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                1,
                changed,
                None,
                "this",
                Some(ip.into())
            )
            .is_ok()
        );
        assert!(
            choose(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                1,
                changed,
                Some(&manual),
                "another",
                None
            )
            .is_err()
        );
    }
    #[test]
    fn wifi_disconnected_ethernet_and_same_subnet_do_not_bypass_route() {
        let ip = Ipv4Addr::new(192, 168, 10, 20);
        let manual = ManualAdapter {
            id: "eth".into(),
            machine: "this".into(),
        };
        let mut list = vec![nic("eth", 1, true, false), nic("wifi", 2, false, true)];
        assert!(
            choose(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                2,
                ip,
                None,
                "this",
                None
            )
            .is_err()
        );
        assert!(
            choose(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                2,
                ip,
                Some(&manual),
                "this",
                None
            )
            .is_err()
        );
        list[0].up = true;
        assert!(
            choose(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                2,
                ip,
                Some(&manual),
                "this",
                None
            )
            .is_err()
        );
        list.push(nic("eth2", 3, true, true));
        assert!(
            choose(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                3,
                ip,
                Some(&manual),
                "this",
                None
            )
            .is_err()
        );
        assert!(
            choose(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                1,
                ip,
                Some(&manual),
                "this",
                None
            )
            .is_ok()
        );
    }
    #[test]
    fn automatic_ignores_wifi_vpn_and_bad_subnets_and_pinned_worker_does_not_switch() {
        let target = Ipv4Addr::new(192, 168, 10, 49);
        let mut eth = nic("eth", 1, true, true);
        let wifi = nic("named Ethernet but Wi-Fi", 2, false, true);
        let vpn = nic("VPN", 3, false, true);
        let mut list = vec![eth.clone(), wifi, vpn];
        assert_eq!(candidate(&list, target, None, "").unwrap().0.id, "eth");
        let pin = ManualAdapter {
            id: "eth".into(),
            machine: "laptop".into(),
        };
        list[0].up = false;
        list.push(nic("replacement-eth", 4, true, true));
        assert!(candidate(&list, target, Some(&pin), "laptop").is_err());
        assert_eq!(
            candidate(&list, target, None, "").unwrap().0.id,
            "replacement-eth"
        );
        eth.prefixes = vec![0];
        assert!(candidate(&[eth.clone()], target, None, "").is_err());
        eth.prefixes = vec![33];
        assert!(candidate(&[eth], target, None, "").is_err());
        assert!(!same_subnet(
            Ipv4Addr::new(192, 168, 10, 20),
            24,
            Ipv4Addr::new(192, 168, 10, 255)
        ));
        assert!(!same_subnet(target, 24, target));
    }

    #[test]
    fn deployment_compatibility_ignores_old_laptop_pin_but_active_pin_stays_strict() {
        let list = vec![nic("new-eth", 1, true, true)];
        let old = ManualAdapter {
            id: "old-eth".into(),
            machine: "old-laptop".into(),
        };
        let effective = deployment_manual(&list, Some(&old), "new-laptop");
        assert!(effective.is_none());
        assert_eq!(
            candidate(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                effective,
                "new-laptop"
            )
            .unwrap()
            .0
            .id,
            "new-eth"
        );
        assert!(
            candidate(
                &list,
                Ipv4Addr::new(192, 168, 10, 49),
                Some(&old),
                "new-laptop"
            )
            .is_err()
        );
        let current = ManualAdapter {
            id: "new-eth".into(),
            machine: "new-laptop".into(),
        };
        assert!(deployment_manual(&list, Some(&current), "new-laptop").is_some());
        assert_eq!(
            candidate(&[], Ipv4Addr::new(192, 168, 10, 49), None, "").unwrap_err(),
            NO_ETHERNET
        );
        let mut wrong = list;
        wrong[0].ipv4[0] = Ipv4Addr::new(192, 168, 20, 20);
        assert_eq!(
            candidate(&wrong, Ipv4Addr::new(192, 168, 10, 49), None, "").unwrap_err(),
            IPV4
        );
    }
    #[cfg(windows)]
    #[test]
    fn windows_inventory_is_read_only_and_has_unique_adapter_ids() {
        let list = adapters().unwrap();
        let mut ids: Vec<_> = list.iter().map(|a| &a.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), list.len());
        assert!(!platform::machine().unwrap().is_empty());
        // Read-only route lookup, no TCP or packets to a physical PLC.
        let (index, ip) = platform::route(Ipv4Addr::LOCALHOST).unwrap();
        assert!(index > 0);
        assert!(ip.is_loopback());
    }

    #[tokio::test]
    async fn automatic_and_resolved_manual_binding_use_loopback_sockets_only() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let automatic = super::super::as200::connect_tcp(
            &endpoint.to_string(),
            None,
            None,
            std::time::Duration::from_secs(1),
        )
        .await
        .unwrap();
        let (_, peer) = listener.accept().await.unwrap();
        assert!(peer.ip().is_loopback());
        drop(automatic);
        let mut adapter = nic("mock-ethernet", 1, true, true);
        adapter.ipv4 = vec![Ipv4Addr::LOCALHOST];
        adapter.prefixes = vec![8];
        let manual = ManualAdapter {
            id: adapter.id.clone(),
            machine: "mock-machine".into(),
        };
        let ip = choose(
            &[adapter],
            Ipv4Addr::new(127, 0, 0, 2),
            1,
            Ipv4Addr::LOCALHOST,
            Some(&manual),
            "mock-machine",
            None,
        )
        .unwrap()
        .unwrap();
        let manual_stream = super::super::as200::bound_socket(endpoint, ip)
            .unwrap()
            .connect(endpoint)
            .await
            .unwrap();
        let (_, peer) = listener.accept().await.unwrap();
        assert_eq!(peer.ip(), ip);
        drop(manual_stream);
        // Invalid source fails before any connect, without panic.
        assert!(
            super::super::as200::bound_socket(endpoint, "192.0.2.77".parse().unwrap()).is_err()
        );
    }
}
