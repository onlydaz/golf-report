use super::*;
use windows_sys::Win32::{
    Foundation::*,
    NetworkManagement::IpHelper::*,
    Networking::WinSock::*,
    System::{Registry::*, Threading::*},
    UI::Shell::*,
};

pub(super) struct Native;
const KEY: &str = "SOFTWARE\\GolfReport\\PlcHostRoutes-v1";
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn ok(code: u32) -> Result<(), u32> {
    if code == 0 { Ok(()) } else { Err(code) }
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn open(write: bool) -> Result<Option<Key>, u32> {
    let mut key = std::ptr::null_mut();
    let path = wide(KEY);
    let result = unsafe {
        if write {
            RegCreateKeyExW(
                HKEY_LOCAL_MACHINE,
                path.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_QUERY_VALUE | KEY_SET_VALUE | KEY_WOW64_64KEY,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        } else {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                path.as_ptr(),
                0,
                KEY_QUERY_VALUE | KEY_WOW64_64KEY,
                &mut key,
            )
        }
    };
    if result == 2 && !write {
        return Ok(None);
    }
    ok(result)?;
    Ok(Some(Key(key)))
}

pub(super) struct OperationLock(HANDLE);
impl Drop for OperationLock {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}
pub(super) fn lock() -> Result<OperationLock, u32> {
    let name = wide("Global\\GolfReportPlcHostRoutes-v1");
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err(unsafe { GetLastError() });
    }
    match unsafe { WaitForSingleObject(handle, 5000) } {
        0 | 128 => Ok(OperationLock(handle)),
        _ => {
            unsafe {
                CloseHandle(handle);
            }
            Err(WORKER)
        }
    }
}

fn address(ip: Ipv4Addr) -> SOCKADDR_INET {
    let mut addr = SOCKADDR_INET::default();
    addr.Ipv4.sin_family = AF_INET;
    addr.Ipv4.sin_addr.S_un.S_addr = u32::from_ne_bytes(ip.octets());
    addr
}
fn row(route: &Route) -> MIB_IPFORWARD_ROW2 {
    let mut row = MIB_IPFORWARD_ROW2::default();
    unsafe {
        InitializeIpForwardEntry(&mut row);
    }
    row.InterfaceIndex = route.index;
    row.InterfaceLuid.Value = route.luid;
    row.DestinationPrefix.Prefix = address(route.target);
    row.DestinationPrefix.PrefixLength = 32;
    row.NextHop = address(route.next_hop);
    row.Metric = route.metric;
    row.Protocol = route.protocol;
    row.SitePrefixLength = route.site_prefix;
    row.ValidLifetime = route.valid;
    row.PreferredLifetime = route.preferred;
    [
        row.Loopback,
        row.AutoconfigureAddress,
        row.Publish,
        row.Immortal,
    ] = route.flags;
    row
}
impl System for Native {
    fn adapters(&self) -> Result<Vec<Adapter>, u32> {
        network::adapters().map_err(|_| CONFIG)
    }
    fn machine(&self) -> Result<String, u32> {
        network::platform::machine().map_err(|_| CONFIG)
    }
    fn best(&self, target: Ipv4Addr) -> Result<(u32, Ipv4Addr), u32> {
        network::platform::route(target).map_err(|_| CONFIG)
    }
    fn hosts(&self, target: Ipv4Addr) -> Result<Vec<Route>, u32> {
        let mut table = std::ptr::null_mut();
        ok(unsafe { GetIpForwardTable2(AF_INET, &mut table) })?;
        if table.is_null() {
            return Err(VERIFY);
        }
        let mut out = Vec::new();
        // OS owns a contiguous table; copy scalar fields before FreeMibTable.
        unsafe {
            for r in
                std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize)
            {
                if r.DestinationPrefix.PrefixLength == 32
                    && r.DestinationPrefix.Prefix.si_family == AF_INET
                    && Ipv4Addr::from(
                        r.DestinationPrefix
                            .Prefix
                            .Ipv4
                            .sin_addr
                            .S_un
                            .S_addr
                            .to_ne_bytes(),
                    ) == target
                {
                    out.push(Route {
                        target,
                        index: r.InterfaceIndex,
                        luid: r.InterfaceLuid.Value,
                        next_hop: Ipv4Addr::from(r.NextHop.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes()),
                        metric: r.Metric,
                        protocol: r.Protocol,
                        site_prefix: r.SitePrefixLength,
                        valid: r.ValidLifetime,
                        preferred: r.PreferredLifetime,
                        flags: [r.Loopback, r.AutoconfigureAddress, r.Publish, r.Immortal],
                        origin: r.Origin,
                    });
                }
            }
            FreeMibTable(table.cast());
        }
        Ok(out)
    }
    fn owned(&self) -> Result<Vec<Owned>, u32> {
        let Some(key) = open(false)? else {
            return Ok(vec![]);
        };
        let mut out = Vec::new();
        for index in 0..256 {
            let mut name = [0u16; 64];
            let mut len = name.len() as u32;
            let result = unsafe {
                RegEnumValueW(
                    key.0,
                    index,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if result == 259 {
                return Ok(out);
            }
            ok(result)?;
            let target: Ipv4Addr = String::from_utf16_lossy(&name[..len as usize])
                .parse()
                .map_err(|_| UNCERTAIN)?;
            let mut data = vec![0u8; 65536];
            let mut size = data.len() as u32;
            ok(unsafe {
                RegGetValueW(
                    key.0,
                    std::ptr::null(),
                    name.as_ptr(),
                    RRF_RT_REG_BINARY,
                    std::ptr::null_mut(),
                    data.as_mut_ptr().cast(),
                    &mut size,
                )
            })?;
            let owner: Owned =
                serde_json::from_slice(&data[..size as usize]).map_err(|_| UNCERTAIN)?;
            if owner.version != 1 || owner.plan.target != target {
                return Err(UNCERTAIN);
            }
            out.push(owner);
        }
        Err(UNCERTAIN)
    }
    fn save(&mut self, value: &Owned) -> Result<(), u32> {
        let key = open(true)?.ok_or(UNCERTAIN)?;
        let name = wide(&value.plan.target.to_string());
        let bytes = serde_json::to_vec(value).map_err(|_| CONFIG)?;
        ok(unsafe {
            RegSetValueExW(
                key.0,
                name.as_ptr(),
                0,
                REG_BINARY,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        })?;
        ok(unsafe { RegFlushKey(key.0) })
    }
    fn forget(&mut self, target: Ipv4Addr) -> Result<(), u32> {
        let key = open(true)?.ok_or(UNCERTAIN)?;
        ok(unsafe { RegDeleteValueW(key.0, wide(&target.to_string()).as_ptr()) })?;
        ok(unsafe { RegFlushKey(key.0) })
    }
    fn create(&mut self, p: &Plan) -> Result<(), u32> {
        // Re-resolve GUID immediately before mutation, rather than trusting an old ifIndex.
        let list = self.adapters()?;
        let a = list
            .iter()
            .find(|a| {
                a.id == p.adapter_id
                    && a.index == p.interface_index
                    && a.ethernet
                    && a.up
                    && a.ipv4.iter().zip(&a.prefixes).any(|(ip, prefix)| {
                        *ip == p.source && network::same_subnet(*ip, *prefix, p.target)
                    })
            })
            .ok_or(CHANGED)?;
        if !self.hosts(p.target)?.is_empty() {
            return Err(CONFLICT);
        }
        let route = Route {
            target: p.target,
            index: a.index,
            luid: a.luid,
            next_hop: Ipv4Addr::UNSPECIFIED,
            metric: 5,
            protocol: MIB_IPPROTO_NETMGMT,
            site_prefix: 0,
            valid: u32::MAX,
            preferred: u32::MAX,
            flags: [false, false, false, true],
            origin: 0,
        };
        match unsafe { CreateIpForwardEntry2(&row(&route)) } {
            183 | 5010 => Err(CONFLICT),
            code => ok(code),
        }
    }
    fn delete(&mut self, r: &Route) -> Result<(), u32> {
        if self.hosts(r.target)?.as_slice() != [r.clone()] {
            return Err(CHANGED);
        }
        ok(unsafe { DeleteIpForwardEntry2(&row(r)) })
    }
}

pub(super) fn elevate(req: &Request) -> Result<(), u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    struct Com(bool);
    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                unsafe {
                    CoUninitialize();
                }
            }
        }
    }
    let hr = unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    // An existing MTA is usable for the synchronous executable/runas operation.
    if hr < 0 && hr != 0x80010106u32 as i32 {
        return Err(VERIFY);
    }
    let _com = Com(hr >= 0);
    let exe = std::env::current_exe().map_err(|_| CONFIG)?;
    let executable: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    let bytes = serde_json::to_vec(req).map_err(|_| CONFIG)?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    if hex.len() > 24000 {
        return Err(CONFIG);
    }
    let parameters = wide(&format!("--plc-host-route {hex}"));
    let verb = wide("runas");
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of_val(&info) as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
    info.lpVerb = verb.as_ptr();
    info.lpFile = executable.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.nShow = 0;
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(unsafe { GetLastError() });
    }
    if info.hProcess.is_null() {
        return Err(VERIFY);
    }
    // Keep the parent's settings gate held until this limited child has exited.
    let wait = unsafe { WaitForSingleObject(info.hProcess, INFINITE) };
    let mut exit = VERIFY;
    let result = if wait == 0 && unsafe { GetExitCodeProcess(info.hProcess, &mut exit) } != 0 {
        ok(exit)
    } else {
        Err(VERIFY)
    };
    unsafe {
        CloseHandle(info.hProcess);
    }
    result
}
