//! Explicit technical access, independent of the Golf decoder and ACK writer.
use super::{connection_settings::Endpoint, worker::WorkerState};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio_modbus::{
    Request, Response, Slave,
    client::{Client, Context, tcp},
};

const TIMEOUT: Duration = Duration::from_secs(3);
const POLICY_ERROR: &str = "Chưa có hồ sơ mapping/thiết bị được xác minh trong plc-register-access.toml cạnh ứng dụng. Chưa được phép thao tác thanh ghi.";
const DISABLED: &str = "Công cụ đọc/ghi thanh ghi kỹ thuật đang tắt. Cần ui_enabled = true ở cấp cao nhất của plc-register-access.toml.";

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum RegisterKind {
    BOOL,
    WORD,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterSelection {
    endpoint: Endpoint,
    kind: RegisterKind,
    address: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Policy {
    version: u8,
    #[serde(default)]
    ui_enabled: bool,
    #[serde(default)]
    profiles: Vec<Profile>,
}

// Read from disk for every operation; never enable from cached frontend state.
// This switch does not access Worker state, PLC, database or runtime files.
fn enabled_policy() -> Result<Policy, String> {
    let path = crate::application_directory()?.join("plc-register-access.toml");
    let text = std::fs::read_to_string(path).map_err(|_| DISABLED)?;
    let policy: Policy = toml::from_str(&text)
        .map_err(|_| "Hồ sơ plc-register-access.toml không hợp lệ; không cấp quyền đọc/ghi.")?;
    if policy.version != 1 {
        return Err(POLICY_ERROR.into());
    }
    if !policy.ui_enabled {
        return Err(DISABLED.into());
    }
    Ok(policy)
}

#[tauri::command]
pub fn get_plc_register_ui_enabled() -> bool {
    enabled_policy().is_ok()
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Profile {
    ip: String,
    port: u32,
    unit_id: u16,
    device_model: String,
    program_id: String,
    mapping_verified: bool,
    // No implicit M0/D0 address. These are PDU bases verified by technical staff.
    m_coil_base: Option<u16>,
    d_register_base: Option<u16>,
    address_offset: i32,
    isolated_test: bool,
    write_enabled: bool,
    #[serde(default)]
    writes: Vec<WritePermission>,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct WritePermission {
    address: String,
    values: Vec<u16>,
    // M bits may control machinery; Golf's control area requires explicit approval.
    #[serde(default)]
    control_write_approved: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterAccess {
    address: String,
    pdu_address: u16,
    address_offset: i32,
    device_model: String,
    program_id: String,
    read_function: u8,
    write_function: u8,
    allowed_values: Vec<u16>,
    write_allowed: bool,
    write_block_reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterRead {
    access: RegisterAccess,
    value: u16,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisterWrite {
    selection: RegisterSelection,
    value: u16,
    expected_before: u16,
    program_id: String,
    mapping_confirmed: bool,
    device_confirmed: bool,
    machine_safe_confirmed: bool,
    pdu_address: u16,
    address_offset: i32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterWriteReport {
    access: RegisterAccess,
    before: u16,
    requested: u16,
    after: Option<u16>,
    // Echo and read-back never imply that Ladder consumed the command.
    write_status: &'static str,
    readback_matches: Option<bool>,
    message: String,
}

fn socket(endpoint: &Endpoint) -> Result<SocketAddr, String> {
    let ip: IpAddr = endpoint
        .ip
        .parse()
        .map_err(|_| "Địa chỉ IP PLC không hợp lệ.")?;
    if !(1..=65535).contains(&endpoint.port) || !(1..=247).contains(&endpoint.unit_id) {
        return Err("Port phải từ 1–65535; Unit ID phải từ 1–247.".into());
    }
    Ok(SocketAddr::new(ip, endpoint.port as u16))
}

fn number(selection: &RegisterSelection) -> Result<u16, String> {
    let (prefix, maximum) = match selection.kind {
        RegisterKind::BOOL => ('M', 8191),
        RegisterKind::WORD => ('D', 29999),
    };
    let text = selection
        .address
        .strip_prefix(prefix)
        .ok_or("Loại và địa chỉ không khớp; dùng M0–M8191 hoặc D0–D29999.")?;
    let n: u16 = text
        .parse()
        .map_err(|_| "Địa chỉ thanh ghi không hợp lệ.")?;
    if n > maximum || selection.address != format!("{prefix}{n}") {
        return Err(
            "Địa chỉ phải viết đúng dạng M0–M8191 hoặc D0–D29999, không có khoảng trắng hay dấu."
                .into(),
        );
    }
    Ok(n)
}

fn resolve(selection: &RegisterSelection) -> Result<(Profile, RegisterAccess), String> {
    let policy = enabled_policy()?;
    let endpoint = socket(&selection.endpoint)?;
    if endpoint.to_string() != super::connection_settings::PLC_ADDRESS
        || selection.endpoint.unit_id != 1
    {
        return Err("Công cụ kỹ thuật chỉ được dùng với PLC cố định trong ứng dụng.".into());
    }
    let n = number(selection)?;
    let mut matches = policy.profiles.into_iter().filter(|p| {
        p.ip.parse::<IpAddr>().ok() == Some(endpoint.ip())
            && p.port == selection.endpoint.port
            && p.unit_id == selection.endpoint.unit_id
    });
    let profile = matches.next().ok_or(POLICY_ERROR)?;
    if matches.next().is_some()
        || !profile.mapping_verified
        || profile.program_id.trim().is_empty()
        || !matches!(profile.device_model.as_str(), "AS228R" | "AS200")
        || !(-1..=1).contains(&profile.address_offset)
    {
        return Err(POLICY_ERROR.into());
    }
    let base = match selection.kind {
        RegisterKind::BOOL => profile.m_coil_base,
        RegisterKind::WORD => profile.d_register_base,
    }
    .ok_or("Chưa xác minh địa chỉ PDU của loại M/D này; không tự giả định địa chỉ bằng 0.")?;
    let pdu = u16::try_from(i32::from(base) + i32::from(n) + profile.address_offset)
        .map_err(|_| "Mapping và offset vượt phạm vi địa chỉ Modbus.")?;
    let permissions: Vec<_> = profile
        .writes
        .iter()
        .filter(|p| p.address == selection.address)
        .collect();
    let sensitive = selection.kind == RegisterKind::BOOL || (25000..=25019).contains(&n);
    let values = permissions
        .first()
        .map(|p| p.values.clone())
        .unwrap_or_default();
    let permitted = permissions.len() == 1
        && !values.is_empty()
        && (!sensitive || permissions[0].control_write_approved)
        && (selection.kind != RegisterKind::BOOL || values.iter().all(|v| *v <= 1));
    let block = if !profile.isolated_test {
        Some("Chưa có interlock chu trình máy được xác minh cho chế độ sản xuất. Chỉ mở ghi trên thiết bị thử nghiệm cách ly đã duyệt.".into())
    } else if !profile.write_enabled || !permitted {
        Some("Địa chỉ/giá trị chưa được cấp quyền ghi trong hồ sơ backend; không mặc định cho phép M0 hoặc D25000.".into())
    } else {
        None
    };
    let access = RegisterAccess {
        address: selection.address.clone(),
        pdu_address: pdu,
        address_offset: profile.address_offset,
        device_model: profile.device_model.clone(),
        program_id: profile.program_id.clone(),
        read_function: if selection.kind == RegisterKind::BOOL {
            1
        } else {
            3
        },
        write_function: if selection.kind == RegisterKind::BOOL {
            5
        } else {
            6
        },
        allowed_values: values,
        write_allowed: block.is_none(),
        write_block_reason: block,
    };
    Ok((profile, access))
}

async fn connect(endpoint: &Endpoint, state: &WorkerState) -> Result<Context, String> {
    enabled_policy()?;
    let address = socket(endpoint)?;
    let directory = state.settings_directory.lock().await.clone()
        .ok_or("Chưa xác định thư mục cấu hình PLC.")?;
    let (config, _) = super::simulator_config::load(Some(&directory))?
        .ok_or("Chưa có cấu hình PLC.")?;
    // The optional source IP belongs only to the configured PLC endpoint.
    // Technical allowlists and write guards remain independently enforced.
    let local_ip = if config.address.parse::<SocketAddr>().ok() == Some(address) {
        config.plc_local_ip
    } else { None };
    let stream = super::as200::connect_tcp(&address.to_string(), local_ip, TIMEOUT)
        .await.map_err(|e| e.to_string())?;
    Ok(tcp::attach_slave(stream, Slave(endpoint.unit_id as u8)))
}

async fn read_value(ctx: &mut Context, kind: RegisterKind, address: u16) -> Result<u16, String> {
    enabled_policy()?;
    let request = match kind {
        RegisterKind::BOOL => Request::ReadCoils(address, 1),
        RegisterKind::WORD => Request::ReadHoldingRegisters(address, 1),
    };
    let response = tokio::time::timeout(TIMEOUT, ctx.call(request))
        .await
        .map_err(|_| "Hết thời gian đọc thanh ghi.")?
        .map_err(|_| "Phản hồi đọc Modbus không hợp lệ hoặc mất kết nối.")?
        .map_err(|_| "PLC từ chối đọc địa chỉ/function code đã chọn.")?;
    match (kind, response) {
        // FC01 carries a whole byte for one coil; tokio-modbus exposes all eight
        // bits. Unused high bits must be zero (Modbus padding), not more coils.
        (RegisterKind::BOOL, Response::ReadCoils(values))
            if values.len() == 8 && values[1..].iter().all(|v| !v) =>
        {
            Ok(u16::from(values[0]))
        }
        (RegisterKind::WORD, Response::ReadHoldingRegisters(values)) if values.len() == 1 => {
            Ok(values[0])
        }
        _ => Err(
            "PLC trả sai function code hoặc số lượng dữ liệu; không dùng kết quả này để ghi."
                .into(),
        ),
    }
}

#[tauri::command]
pub async fn get_plc_register_access(
    selection: RegisterSelection,
    state: tauri::State<'_, WorkerState>,
) -> Result<RegisterAccess, String> {
    let (_, mut access) = resolve(&selection)?;
    let _gate = state
        .settings_gate
        .try_lock()
        .map_err(|_| "Đang thực hiện thao tác PLC khác.")?;
    if let Err(reason) = state.ensure_register_writes_stopped().await {
        access.write_allowed = false;
        access.write_block_reason = Some(reason);
    }
    Ok(access)
}

#[tauri::command]
pub async fn read_plc_register(
    selection: RegisterSelection,
    state: tauri::State<'_, WorkerState>,
) -> Result<RegisterRead, String> {
    let _gate = state
        .settings_gate
        .try_lock()
        .map_err(|_| "Đang thực hiện thao tác PLC khác.")?;
    let (_, access) = resolve(&selection)?;
    let mut ctx = connect(&selection.endpoint, &state).await?;
    let value = read_value(&mut ctx, selection.kind, access.pdu_address).await?;
    Ok(RegisterRead { access, value })
}

#[tauri::command]
pub async fn write_plc_register(
    request: RegisterWrite,
    state: tauri::State<'_, WorkerState>,
) -> Result<RegisterWriteReport, String> {
    enabled_policy()?;
    // This gate is also held by stop/start/configuration commands. Never release
    // it between verifying the stopped Worker and sending/read-backing the write.
    let _gate = state
        .settings_gate
        .try_lock()
        .map_err(|_| "Đang thực hiện thao tác PLC khác.")?;
    state.ensure_register_writes_stopped().await?;
    let (profile, access) = resolve(&request.selection)?;
    if !access.write_allowed || !access.allowed_values.contains(&request.value) {
        return Err(access
            .write_block_reason
            .unwrap_or("Giá trị không có trong danh sách cho phép của backend.".into()));
    }
    if !request.mapping_confirmed
        || !request.device_confirmed
        || !request.machine_safe_confirmed
        || request.program_id != profile.program_id
        || request.pdu_address != access.pdu_address
        || request.address_offset != access.address_offset
    {
        return Err("Cần xác nhận đúng thiết bị/chương trình, mapping và máy thử nghiệm đã cách ly, không có chu trình nguy hiểm trước mỗi lần ghi.".into());
    }
    // Manual writes are not an alternative path into the Golf ACK/control block.
    let d_base = profile
        .d_register_base
        .ok_or("Chưa xác minh mapping D để loại trừ chương trình Golf; khóa ghi.")?;
    let header = u16::try_from(i32::from(d_base) + 25000 + profile.address_offset)
        .map_err(|_| "Địa chỉ kiểm tra chương trình không hợp lệ.")?;
    let mut ctx = connect(&request.selection.endpoint, &state).await?;
    enabled_policy()?;
    let response =
        tokio::time::timeout(TIMEOUT, ctx.call(Request::ReadHoldingRegisters(header, 3)))
            .await
            .map_err(|_| "Không đọc được vùng nhận dạng Golf; chưa gửi lệnh ghi.")?
            .map_err(|_| "Không xác minh được chương trình PLC; chưa gửi lệnh ghi.")?
            .map_err(|_| "PLC từ chối kiểm tra chương trình; chưa gửi lệnh ghi.")?;
    match response {
        Response::ReadHoldingRegisters(words) if words.len() == 3 => {
            if words == [1, 240, 2] {
                return Err("PLC đang công bố Register Map Golf. Khóa ghi kỹ thuật; không sử dụng công cụ này để vượt guard ACK/Journal.".into());
            }
        }
        _ => return Err("Phản hồi kiểm tra chương trình không hợp lệ; chưa gửi lệnh ghi.".into()),
    }
    let before = read_value(&mut ctx, request.selection.kind, access.pdu_address).await?;
    if before != request.expected_before {
        return Err("Giá trị PLC đã đổi sau lần đọc/xác nhận. Chưa ghi; hãy đọc lại và xác nhận thao tác mới.".into());
    }
    let (current_profile, _) = resolve(&request.selection)?;
    if current_profile != profile {
        return Err(
            "Hồ sơ kỹ thuật đã thay đổi trong lúc kiểm tra PLC. Chưa ghi; hãy đọc và xác nhận lại."
                .into(),
        );
    }
    state.ensure_register_writes_stopped().await?;
    let (write, expected) = match request.selection.kind {
        RegisterKind::BOOL => (
            Request::WriteSingleCoil(access.pdu_address, request.value == 1),
            Response::WriteSingleCoil(access.pdu_address, request.value == 1),
        ),
        RegisterKind::WORD => (
            Request::WriteSingleRegister(access.pdu_address, request.value),
            Response::WriteSingleRegister(access.pdu_address, request.value),
        ),
    };
    // Exactly one write call. Timeout/disconnect/malformed echo never triggers retry.
    let (write_status, message) = match tokio::time::timeout(TIMEOUT, ctx.call(write)).await {
        Ok(Ok(Ok(response))) if response == expected => (
            "ECHO_CONFIRMED",
            "Đã nhận phản hồi Modbus đúng. Cần quan sát ISPSoft để xác nhận Ladder thực sự xử lý.",
        ),
        Ok(Ok(Err(_))) => (
            "REFUSED",
            "PLC trả exception cho lệnh ghi. Không tự gửi lại; kiểm tra quyền và chương trình trên ISPSoft.",
        ),
        _ => (
            "UNCONFIRMED",
            "Kết quả ghi chưa xác định do mất kết nối, timeout hoặc phản hồi sai. Có thể PLC đã nhận lệnh; không tự gửi lại. Đối chiếu trên ISPSoft.",
        ),
    };
    // Drop any uncertain socket; only a new, read-only request is attempted.
    drop(ctx);
    let after = match connect(&request.selection.endpoint, &state).await {
        Ok(mut reader) => read_value(&mut reader, request.selection.kind, access.pdu_address)
            .await
            .ok(),
        Err(_) => None,
    };
    Ok(RegisterWriteReport {
        readback_matches: after.map(|v| v == request.value),
        access,
        before,
        requested: request.value,
        after,
        write_status,
        message: message.into(),
    })
}
