//! Explicit, narrowly scoped /32 route operation. Never called by polling/reconnect.
use super::network::{self, Adapter, ManualAdapter};
use serde::{Deserialize, Serialize};
use std::{net::Ipv4Addr, path::PathBuf};

const CONFLICT: u32 = 183;
const CHANGED: u32 = 10001;
const UNCERTAIN: u32 = 10002;
const CONFIG: u32 = 10003;
const WORKER: u32 = 10004;
const VERIFY: u32 = 10005;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Plan {
    pub target: Ipv4Addr,
    pub adapter_id: String,
    pub adapter_name: String,
    pub interface_index: u32,
    pub source: Ipv4Addr,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Route {
    target: Ipv4Addr,
    index: u32,
    luid: u64,
    next_hop: Ipv4Addr,
    metric: u32,
    protocol: i32,
    site_prefix: u8,
    valid: u32,
    preferred: u32,
    flags: [bool; 4],
    origin: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owned {
    version: u8,
    plan: Plan,
    machine: String,
    committed: bool,
    route: Option<Route>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub message: Option<String>,
    pub repair: Option<Plan>,
    pub owned: Vec<Plan>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    remove: bool,
    plan: Plan,
    machine: String,
    config_path: PathBuf,
    configured_address: String,
    configured_adapter: Option<ManualAdapter>,
}

// The production implementation uses IP Helper + admin-protected HKLM ownership.
// Unit tests implement this boundary entirely in memory: no OS route mutation.
trait System {
    fn adapters(&self) -> Result<Vec<Adapter>, u32>;
    fn machine(&self) -> Result<String, u32>;
    fn best(&self, target: Ipv4Addr) -> Result<(u32, Ipv4Addr), u32>;
    fn hosts(&self, target: Ipv4Addr) -> Result<Vec<Route>, u32>;
    fn owned(&self) -> Result<Vec<Owned>, u32>;
    fn save(&mut self, value: &Owned) -> Result<(), u32>;
    fn forget(&mut self, target: Ipv4Addr) -> Result<(), u32>;
    fn create(&mut self, plan: &Plan) -> Result<(), u32>;
    fn delete(&mut self, route: &Route) -> Result<(), u32>;
}

fn execute(os: &mut impl System, req: &Request) -> Result<(), u32> {
    let p = &req.plan;
    // Limited helper: one RFC1918 unicast host, on-link, physical Ethernet only.
    if !p.target.is_private() || req.machine != os.machine()? {
        return Err(CONFIG);
    }
    let hosts = os.hosts(p.target)?;
    let owned = os.owned()?.into_iter().find(|o| o.plan.target == p.target);
    if req.remove {
        let owner = owned.ok_or(UNCERTAIN)?;
        if owner.version != 1 || owner.machine != req.machine || owner.plan != *p {
            return Err(CHANGED);
        }
        if hosts.is_empty() {
            return os.forget(p.target);
        } // expired/reboot: no route to delete
        if !owner.committed {
            return Err(UNCERTAIN);
        }
        if hosts.len() != 1 || owner.route.as_ref() != hosts.first() {
            return Err(CHANGED);
        }
        os.delete(&hosts[0])?;
        if !os.hosts(p.target)?.is_empty() {
            return Err(VERIFY);
        }
        return os.forget(p.target);
    }
    if !hosts.is_empty()
        || owned.as_ref().is_some_and(|o| {
            !o.committed || o.version != 1 || o.machine != req.machine || o.plan != *p
        })
    {
        return Err(CONFLICT);
    }
    let adapters = os.adapters()?;
    let manual = ManualAdapter {
        id: p.adapter_id.clone(),
        machine: req.machine.clone(),
    };
    let (adapter, ip) =
        network::candidate(&adapters, p.target, Some(&manual), &req.machine).map_err(|_| CONFIG)?;
    if adapter.index != p.interface_index || ip != p.source || adapter.name != p.adapter_name {
        return Err(CHANGED);
    }
    if os
        .best(p.target)
        .is_ok_and(|(index, _)| index == p.interface_index)
    {
        return Err(CHANGED);
    }
    // Intent is persisted BEFORE the OS call. A crash in this window cannot
    // turn an unrelated pre-existing /32 into an owned, deletable route.
    let mut owner = Owned {
        version: 1,
        plan: p.clone(),
        machine: req.machine.clone(),
        committed: false,
        route: None,
    };
    os.save(&owner)?;
    os.create(p)?; // Create, never Set/replace. A racing duplicate is an error.
    let rows = os.hosts(p.target)?;
    let row = rows.first().ok_or(VERIFY)?;
    if rows.len() != 1
        || row.index != p.interface_index
        || row.next_hop != Ipv4Addr::UNSPECIFIED
        || row.metric != 5
        || row.protocol != 3
    {
        return Err(VERIFY);
    }
    owner.route = Some(row.clone());
    owner.committed = true;
    os.save(&owner)?;
    let (index, source) = os.best(p.target)?;
    if index != p.interface_index || !adapter.ipv4.contains(&source) {
        return Err(VERIFY);
    }
    Ok(())
}

fn describe(code: u32) -> String {
    match code {
        5 | 1223 => "Không được cấp quyền sửa tuyến (UAC bị hủy hoặc không có Administrator). Nhờ kỹ thuật tạo host route /32 on-link tới PLC qua Ethernet đã chọn; không đổi gateway/Wi-Fi.".into(),
        CONFLICT => "Đã có host route hoặc bằng chứng sửa tuyến tới IP PLC. Không tạo trùng hoặc ghi đè. Hoàn tác tuyến Golf Report cũ, hoặc nhờ kỹ thuật kiểm tra route có sẵn.".into(),
        CHANGED => "Adapter, cấu hình hoặc tuyến đã thay đổi từ lúc xác nhận. Không ghi đè/xóa route; cập nhật card mạng và kiểm tra lại.".into(),
        UNCERTAIN => "Không đủ bằng chứng sở hữu tuyến để xóa an toàn. Nhờ kỹ thuật đối chiếu; ứng dụng không tự xóa route hiện có.".into(),
        CONFIG => "Ethernet/IP/subnet hoặc cấu hình PLC không còn phù hợp. Chọn lại card và kiểm tra IPv4 trước khi sửa tuyến.".into(),
        WORKER => "Bộ thu có thể đang chạy hoặc khóa thu không còn xác minh được. Dừng Worker hoàn toàn trước khi sửa tuyến.".into(),
        VERIFY => "Thao tác tuyến chưa được xác minh hoàn tất. Đọc lại trạng thái; chỉ hoàn tác tuyến có bằng chứng sở hữu, không gửi ACK để thử mạng.".into(),
        _ => format!("Windows không hoàn tất thao tác tuyến (mã {code}). Giữ nguyên cấu hình, cập nhật trạng thái hoặc liên hệ kỹ thuật."),
    }
}

pub fn inspect(config: &super::simulator_config::SimulatorConfig) -> Result<Status, String> {
    inspect_with(&native::Native, config)
}

fn inspect_with(
    os: &impl System,
    config: &super::simulator_config::SimulatorConfig,
) -> Result<Status, String> {
    let owned = os
        .owned()
        .map_err(describe)?
        .into_iter()
        .map(|o| o.plan)
        .collect();
    let mut status = Status {
        owned,
        message: None,
        repair: None,
    };
    let endpoint: std::net::SocketAddr = config.address.parse().map_err(|_| describe(CONFIG))?;
    if endpoint.ip().is_loopback() {
        return Ok(status);
    }
    let std::net::IpAddr::V4(target) = endpoint.ip() else {
        return Err(describe(CONFIG));
    };
    let list = os.adapters().map_err(describe)?;
    let machine = os.machine().map_err(describe)?;
    let (a, source) = match network::candidate(
        &list,
        target,
        network::deployment_manual(&list, config.plc_adapter.as_ref(), &machine),
        &machine,
    ) {
        Ok(pair) => pair,
        Err(message) => {
            status.message = Some(message);
            return Ok(status);
        }
    };
    if os.best(target).is_ok_and(|(index, _)| index == a.index) {
        return Ok(status);
    }
    let proposed = Plan {
        target,
        adapter_id: a.id.clone(),
        adapter_name: a.name.clone(),
        interface_index: a.index,
        source,
    };
    let evidence = os.owned().map_err(describe)?;
    if !os.hosts(target).map_err(describe)?.is_empty()
        || evidence.iter().any(|o| {
            o.plan.target == target && (!o.committed || o.plan != proposed || o.machine != machine)
        })
    {
        status.message = Some(describe(CONFLICT));
        return Ok(status);
    }
    if !target.is_private() {
        status.message =
            Some("Chỉ hỗ trợ đề xuất host route cho địa chỉ PLC nội bộ. Liên hệ kỹ thuật.".into());
        return Ok(status);
    }
    status.message = Some("Windows chưa chọn đúng Ethernet tới PLC. Có thể tạo đường mạng riêng cho IP PLC mà không tắt Wi-Fi. Subnet không xác nhận cáp PLC; hãy kiểm tra card được đề xuất.".into());
    status.repair = Some(proposed);
    Ok(status)
}

pub fn change(
    config: &super::simulator_config::SimulatorConfig,
    config_path: PathBuf,
    plan: Plan,
    remove: bool,
) -> Result<(), String> {
    let status = inspect(config)?;
    if (!remove && status.repair.as_ref() != Some(&plan))
        || (remove && !status.owned.contains(&plan))
    {
        return Err(describe(CHANGED));
    }
    // Unelevated parent only prepares the existing acquisition lock location.
    let lock_path = config.journal_file(&config_path).with_extension("lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    drop(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|e| e.to_string())?,
    );
    let req = Request {
        remove,
        plan,
        machine: network::platform::machine()?,
        config_path,
        configured_address: config.address.clone(),
        configured_adapter: config.plc_adapter.clone(),
    };
    native::elevate(&req).map_err(describe)
}

/// Must run BEFORE Tauri setup: this child never opens DB, Modbus or Worker.
pub fn helper_entry() -> Option<i32> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("--plc-host-route") {
        return None;
    }
    Some(
        (|| {
            if args.len() != 3 || args[2].len() > 24000 || args[2].len() % 2 != 0 {
                return Err(CONFIG);
            }
            let bytes: Result<Vec<_>, _> = args[2]
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    std::str::from_utf8(pair)
                        .ok()
                        .and_then(|s| u8::from_str_radix(s, 16).ok())
                        .ok_or(CONFIG)
                })
                .collect();
            let req: Request = serde_json::from_slice(&bytes?).map_err(|_| CONFIG)?;
            let expected = crate::application_directory()
                .map_err(|_| CONFIG)?
                .join("as200_config.toml");
            if !req.config_path.is_absolute() || req.config_path != expected {
                return Err(CONFIG);
            }
            #[cfg(windows)]
            let _operation = native::lock()?;
            let text = std::fs::read_to_string(&req.config_path).map_err(|_| CONFIG)?;
            let cfg: super::simulator_config::SimulatorConfig =
                toml::from_str(&text).map_err(|_| CONFIG)?;
            super::connection_settings::validate_config(&cfg).map_err(|_| CONFIG)?;
            if cfg.address != req.configured_address || cfg.plc_adapter != req.configured_adapter {
                return Err(CHANGED);
            }
            if !req.remove
                && cfg
                    .address
                    .parse::<std::net::SocketAddr>()
                    .map_err(|_| CONFIG)?
                    .ip()
                    != req.plan.target
            {
                return Err(CONFIG);
            }
            // Open only, never create/write arbitrary files with elevated privilege.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .open(cfg.journal_file(&req.config_path).with_extension("lock"))
                .map_err(|_| WORKER)?;
            file.try_lock().map_err(|_| WORKER)?;
            let result = execute(&mut native::Native, &req);
            drop(file);
            result
        })()
        .err()
        .unwrap_or(0) as i32,
    )
}

#[cfg(windows)]
mod native;
#[cfg(not(windows))]
mod native {
    use super::*;
    pub struct Native;
    impl System for Native {
        fn adapters(&self) -> Result<Vec<Adapter>, u32> {
            Err(CONFIG)
        }
        fn machine(&self) -> Result<String, u32> {
            Err(CONFIG)
        }
        fn best(&self, _: Ipv4Addr) -> Result<(u32, Ipv4Addr), u32> {
            Err(CONFIG)
        }
        fn hosts(&self, _: Ipv4Addr) -> Result<Vec<Route>, u32> {
            Err(CONFIG)
        }
        fn owned(&self) -> Result<Vec<Owned>, u32> {
            Ok(vec![])
        }
        fn save(&mut self, _: &Owned) -> Result<(), u32> {
            Err(CONFIG)
        }
        fn forget(&mut self, _: Ipv4Addr) -> Result<(), u32> {
            Err(CONFIG)
        }
        fn create(&mut self, _: &Plan) -> Result<(), u32> {
            Err(CONFIG)
        }
        fn delete(&mut self, _: &Route) -> Result<(), u32> {
            Err(CONFIG)
        }
    }
    pub(super) fn elevate(_: &Request) -> Result<(), u32> {
        Err(CONFIG)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone)]
    struct Mock {
        list: Vec<Adapter>,
        best: u32,
        routes: Vec<Route>,
        owners: Vec<Owned>,
        deny: bool,
        writes: usize,
        fail_save: bool,
    }
    fn setup() -> (Mock, Request) {
        let source = Ipv4Addr::new(192, 168, 10, 20);
        let adapter = Adapter {
            id: "physical-eth".into(),
            name: "Ethernet mock".into(),
            index: 7,
            luid: 7,
            ethernet: true,
            up: true,
            ipv4: vec![source],
            prefixes: vec![24],
        };
        let plan = Plan {
            target: Ipv4Addr::new(192, 168, 10, 49),
            adapter_id: adapter.id.clone(),
            adapter_name: adapter.name.clone(),
            interface_index: 7,
            source,
        };
        (
            Mock {
                list: vec![adapter],
                best: 12,
                routes: vec![],
                owners: vec![],
                deny: false,
                writes: 0,
                fail_save: false,
            },
            Request {
                remove: false,
                plan,
                machine: "laptop".into(),
                config_path: PathBuf::new(),
                configured_address: String::new(),
                configured_adapter: None,
            },
        )
    }
    impl System for Mock {
        fn adapters(&self) -> Result<Vec<Adapter>, u32> {
            Ok(self.list.clone())
        }
        fn machine(&self) -> Result<String, u32> {
            Ok("laptop".into())
        }
        fn best(&self, _: Ipv4Addr) -> Result<(u32, Ipv4Addr), u32> {
            Ok((self.best, self.list[0].ipv4[0]))
        }
        fn hosts(&self, t: Ipv4Addr) -> Result<Vec<Route>, u32> {
            Ok(self
                .routes
                .iter()
                .filter(|r| r.target == t)
                .cloned()
                .collect())
        }
        fn owned(&self) -> Result<Vec<Owned>, u32> {
            Ok(self.owners.clone())
        }
        fn save(&mut self, o: &Owned) -> Result<(), u32> {
            if self.fail_save {
                return Err(5);
            }
            self.owners.retain(|v| v.plan.target != o.plan.target);
            self.owners.push(o.clone());
            Ok(())
        }
        fn forget(&mut self, t: Ipv4Addr) -> Result<(), u32> {
            self.owners.retain(|o| o.plan.target != t);
            Ok(())
        }
        fn create(&mut self, p: &Plan) -> Result<(), u32> {
            assert!(self.owners.iter().any(|o| o.plan == *p && !o.committed));
            if self.deny {
                return Err(5);
            }
            self.writes += 1;
            self.best = p.interface_index;
            self.routes.push(Route {
                target: p.target,
                index: p.interface_index,
                luid: 7,
                next_hop: Ipv4Addr::UNSPECIFIED,
                metric: 5,
                protocol: 3,
                site_prefix: 0,
                valid: u32::MAX,
                preferred: u32::MAX,
                flags: [false, false, false, true],
                origin: 0,
            });
            Ok(())
        }
        fn delete(&mut self, r: &Route) -> Result<(), u32> {
            self.writes += 1;
            self.routes.retain(|v| v != r);
            self.best = 12;
            Ok(())
        }
    }
    #[test]
    fn wrong_wifi_route_is_repaired_and_exact_owned_route_can_be_undone() {
        let (mut os, mut req) = setup();
        execute(&mut os, &req).unwrap();
        assert_eq!(os.writes, 1);
        assert_eq!(os.best, 7);
        assert!(os.owners[0].committed);
        assert_eq!(execute(&mut os, &req), Err(CONFLICT));
        assert_eq!(os.writes, 1);
        req.remove = true;
        execute(&mut os, &req).unwrap();
        assert!(os.routes.is_empty() && os.owners.is_empty());
        assert_eq!(os.writes, 2);
    }
    #[test]
    fn already_correct_route_and_existing_foreign_host_never_get_modified() {
        let (mut os, req) = setup();
        os.best = 7;
        assert_eq!(execute(&mut os, &req), Err(CHANGED));
        assert_eq!(os.writes, 0);
        os.best = 12;
        execute(&mut os, &req).unwrap();
        os.owners.clear();
        os.best = 12;
        assert_eq!(execute(&mut os, &req), Err(CONFLICT));
        assert_eq!(os.writes, 1);
    }
    #[test]
    fn administrator_denied_or_intent_save_failure_never_creates_a_route() {
        let (mut os, mut req) = setup();
        os.deny = true;
        assert_eq!(execute(&mut os, &req), Err(5));
        assert_eq!(os.writes, 0);
        assert!(os.routes.is_empty());
        assert!(!os.owners[0].committed);
        req.remove = true;
        execute(&mut os, &req).unwrap();
        assert!(os.owners.is_empty());
        req.remove = false;
        os.fail_save = true;
        assert_eq!(execute(&mut os, &req), Err(5));
        assert_eq!(os.writes, 0);
        assert!(describe(1223).contains("UAC"));
    }
    #[test]
    fn changed_route_unknown_intent_and_duplicate_route_are_never_deleted() {
        for fault in ["changed", "unknown", "duplicate"] {
            let (mut os, mut req) = setup();
            execute(&mut os, &req).unwrap();
            req.remove = true;
            match fault {
                "changed" => os.routes[0].metric += 1,
                "unknown" => os.owners[0].committed = false,
                _ => os.routes.push(os.routes[0].clone()),
            }
            assert!(execute(&mut os, &req).is_err());
            assert_eq!(os.writes, 1);
            assert!(!os.routes.is_empty());
        }
    }
    #[test]
    fn disconnected_changed_ipv4_new_laptop_and_changed_interface_reject_stale_consent() {
        for fault in ["down", "ip", "machine", "index"] {
            let (mut os, mut req) = setup();
            match fault {
                "down" => os.list[0].up = false,
                "ip" => os.list[0].ipv4[0] = Ipv4Addr::new(192, 168, 10, 21),
                "machine" => req.machine = "old-laptop".into(),
                _ => os.list[0].index = 9,
            }
            assert!(execute(&mut os, &req).is_err());
            assert_eq!(os.writes, 0);
        }
    }
    #[test]
    fn expired_owned_route_can_be_explicitly_restored_or_forgotten_after_reboot() {
        let (mut os, mut req) = setup();
        execute(&mut os, &req).unwrap();
        os.routes.clear();
        os.best = 12;
        execute(&mut os, &req).unwrap();
        assert_eq!(os.writes, 2);
        os.routes.clear();
        req.remove = true;
        execute(&mut os, &req).unwrap();
        assert!(os.owners.is_empty());
        assert_eq!(os.writes, 2);
    }
    #[test]
    fn old_target_is_undoable_after_config_changes_and_public_targets_are_refused() {
        let (mut os, mut req) = setup();
        execute(&mut os, &req).unwrap();
        req.configured_address = "192.168.20.49:502".into();
        req.remove = true;
        execute(&mut os, &req).unwrap();
        assert!(os.routes.is_empty());
        req.remove = false;
        req.plan.target = Ipv4Addr::new(8, 8, 8, 8);
        assert_eq!(execute(&mut os, &req), Err(CONFIG));
        assert_eq!(os.writes, 2);
    }
    #[test]
    fn read_only_proposal_requires_unique_suitable_ethernet_and_wrong_route() {
        let (mut os, req) = setup();
        let mut cfg = super::super::simulator_config::SimulatorConfig {
            address: format!("{}:502", req.plan.target),
            ..Default::default()
        };
        assert_eq!(
            inspect_with(&os, &cfg).unwrap().repair,
            Some(req.plan.clone())
        );
        assert_eq!(os.writes, 0); // inspecting must not create or send anything
        os.best = 7;
        assert!(inspect_with(&os, &cfg).unwrap().repair.is_none());
        os.best = 12;
        let mut other = os.list[0].clone();
        other.id = "another-ethernet".into();
        other.index = 8;
        os.list.push(other);
        let status = inspect_with(&os, &cfg).unwrap();
        assert!(status.repair.is_none());
        assert!(status.message.unwrap().contains("Nhiều Ethernet"));
        cfg.plc_adapter = Some(ManualAdapter {
            id: req.plan.adapter_id.clone(),
            machine: "laptop".into(),
        });
        assert_eq!(
            inspect_with(&os, &cfg).unwrap().repair,
            Some(req.plan.clone())
        );
        os.list[0].up = false;
        assert!(inspect_with(&os, &cfg).unwrap().repair.is_none());
        assert_eq!(os.writes, 0);
    }
}
