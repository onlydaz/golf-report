//! AS200 Published Record: one read region and a durable, explicitly authorized ACK handshake.

pub mod ack;
pub mod decode;
pub mod heartbeat;
pub mod map;
pub mod model;
pub mod reader;
#[cfg(test)]
mod tests;

pub use model::WordOrder;
use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio::net::{TcpSocket, TcpStream};

const ROUTE_HELP: &str = "Bind IP nguồn không bảo đảm Windows chọn Ethernet. Kiểm tra IP Ethernet và route/interface metric tới PLC; ứng dụng không tự đổi cấu hình mạng.";

pub(crate) fn validate_local_ip(endpoint: SocketAddr, local_ip: IpAddr) -> Result<(), String> {
    if endpoint.is_ipv4() != local_ip.is_ipv4()
        || local_ip.is_unspecified()
        || local_ip.is_multicast()
        || local_ip == IpAddr::V4(std::net::Ipv4Addr::BROADCAST)
        || endpoint.ip().is_loopback() != local_ip.is_loopback()
    {
        return Err("IP nguồn PLC phải là IP unicast cùng họ IPv4/IPv6 và phù hợp với endpoint. Chọn IP Ethernet của laptop; không dùng loopback cho PLC từ xa.".into());
    }
    Ok(())
}

// Binding proves the IP is assigned locally (or the OS rejects it). It does not
// identify an adapter as Ethernet, select an interface, or change the route table.
pub(crate) fn bound_socket(endpoint: SocketAddr, local_ip: IpAddr) -> Result<TcpSocket, String> {
    validate_local_ip(endpoint, local_ip)?;
    let socket = if endpoint.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }
    .map_err(|e| format!("Không tạo được socket PLC: {e}"))?;
    socket.bind(SocketAddr::new(local_ip, 0)).map_err(|e| {
        eprintln!("PLC_TCP state=BIND_FAILED endpoint={endpoint} local_ip={local_ip} kind={:?} error={e}", e.kind());
        format!("Không bind được IP nguồn PLC {local_ip}. Kiểm tra IP này đang được gán trên Ethernet và card đã bật. Lỗi hệ điều hành: {e}. {ROUTE_HELP}")
    })?;
    Ok(socket)
}

pub(crate) async fn connect_tcp(
    address: &str,
    local_ip: Option<IpAddr>,
    manual: Option<&super::network::ManualAdapter>,
    duration: Duration,
) -> Result<TcpStream, As200Error> {
    let endpoint: SocketAddr = address
        .parse()
        .map_err(|_| As200Error::Transport("Địa chỉ PLC phải là IP:port dạng số.".into()))?;
    let local_ip =
        super::network::resolve(endpoint, local_ip, manual).map_err(As200Error::Transport)?;
    let selected = local_ip
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "auto".into());
    let connection = async {
        match local_ip {
            Some(ip) => {
                bound_socket(endpoint, ip)
                    .map_err(As200Error::Transport)?
                    .connect(endpoint)
                    .await
            }
            None => TcpStream::connect(endpoint).await,
        }
        .map_err(|e| {
            eprintln!(
                "PLC_TCP state=FAILED endpoint={endpoint} local_ip={selected} kind={:?} error={e}",
                e.kind()
            );
            let reason = match e.kind() {
                std::io::ErrorKind::ConnectionRefused => "PLC từ chối kết nối TCP",
                std::io::ErrorKind::TimedOut => "Hết thời gian kết nối TCP PLC",
                _ => "Không kết nối được TCP PLC",
            };
            As200Error::Transport(format!(
                "{reason} ({endpoint}, IP nguồn {selected}): {e}. {ROUTE_HELP}"
            ))
        })
    };
    let stream = tokio::time::timeout(duration, connection)
        .await
        .map_err(|_| {
            eprintln!(
                "PLC_TCP state=TIMEOUT endpoint={endpoint} local_ip={selected} timeout_ms={}",
                duration.as_millis()
            );
            As200Error::Transport(format!(
                "Hết thời gian kết nối TCP PLC {endpoint}, IP nguồn {selected}. {ROUTE_HELP}"
            ))
        })??;
    eprintln!(
        "PLC_TCP state=CONNECTED endpoint={endpoint} local_ip={selected} actual_local={:?}",
        stream.local_addr().ok()
    );
    Ok(stream)
}

#[derive(Debug, Clone)]
pub struct As200Config {
    pub network_adapter: Option<super::network::ManualAdapter>,
    pub control_start: u16,
    pub record_size: usize,
    pub buffer_count: usize,
    pub address: String,
    pub unit_id: u8,
    // TODO kỹ thuật viên xác nhận ánh xạ D -> địa chỉ PDU; tài liệu đề xuất 0.
    pub modbus_address_offset: i32,
    // TODO xác nhận thứ tự WORD cho cả DINT32 và REAL32 trên thiết bị.
    pub word_order: WordOrder,
    pub timeout: Duration,
}
// Cố ý không có Default: caller phải chọn địa chỉ, offset và WordOrder rõ ràng.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum As200Error {
    InvalidData(String),
    Transport(String),
    /// A PLC response was received, but it rejected or violated FC03.
    Protocol(String),
    Timeout,
    Unstable,
}
impl fmt::Display for As200Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidData(detail) => write!(f, "Dữ liệu AS200 không hợp lệ: {detail}"),
            Self::Transport(detail) => write!(f, "Lỗi Modbus TCP AS200: {detail}"),
            Self::Protocol(detail) => write!(f, "Lỗi phản hồi Modbus AS200: {detail}"),
            Self::Timeout => write!(f, "Hết thời gian kết nối/đọc AS200"),
            Self::Unstable => write!(f, "Buffer AS200 đang thay đổi; bỏ snapshot và thử lại"),
        }
    }
}
impl std::error::Error for As200Error {}
