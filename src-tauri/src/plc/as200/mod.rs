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

// Probe, FC03 and prepared ACK sockets share this optional source bind.
// Binding does not alter the operating system's routes or select an adapter.
pub(crate) async fn connect_tcp(
    address: &str,
    local_bind_ip: Option<IpAddr>,
    duration: Duration,
) -> Result<TcpStream, As200Error> {
    let endpoint: SocketAddr = address
        .parse()
        .map_err(|_| As200Error::Transport("Địa chỉ PLC phải là IP:port dạng số.".into()))?;
    let source = local_bind_ip
        .map(|ip| ip.to_string())
        .unwrap_or_else(|| "auto".into());
    let connection = async {
        let result = match local_bind_ip {
            Some(ip) => {
                if ip.is_ipv4() != endpoint.is_ipv4()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip == IpAddr::V4(std::net::Ipv4Addr::BROADCAST)
                {
                    return Err(As200Error::Transport(format!(
                        "local_bind_ip={ip} phải là IP unicast cụ thể, cùng họ IPv4/IPv6 với PLC {endpoint}."
                    )));
                }
                let socket = if ip.is_ipv4() {
                    TcpSocket::new_v4()
                } else {
                    TcpSocket::new_v6()
                }
                .map_err(|e| {
                    super::diagnostics::note_tcp_failure(endpoint, local_bind_ip);
                    As200Error::Transport(format!("Không tạo được socket PLC để bind {ip}: {e}"))
                })?;
                socket.bind(SocketAddr::new(ip, 0)).map_err(|e| {
                    super::diagnostics::note_tcp_failure(endpoint, local_bind_ip);
                    crate::plc::diagnostics::event!("PLC_TCP state=BIND_FAILED endpoint={endpoint} local_bind_ip={ip} error={e}");
                    As200Error::Transport(format!(
                        "Không bind được IP nguồn {ip} trước khi kết nối PLC {endpoint}: {e}. Kiểm tra IP này đã được gán cho laptop."
                    ))
                })?;
                socket.connect(endpoint).await
            }
            None => TcpStream::connect(endpoint).await,
        };
        result.map_err(|e| {
            super::diagnostics::note_tcp_failure(endpoint, local_bind_ip);
            crate::plc::diagnostics::event!(
                "PLC_TCP state=FAILED endpoint={endpoint} source_ip={source} routing=system kind={:?} error={e}",
                e.kind()
            );
            let reason = match e.kind() {
                std::io::ErrorKind::ConnectionRefused => "PLC từ chối kết nối TCP",
                std::io::ErrorKind::TimedOut => "Hết thời gian kết nối TCP PLC",
                _ => "Không kết nối được TCP PLC",
            };
            As200Error::Transport(format!(
                "{reason} ({endpoint}, IP nguồn {source}): {e}. Kiểm tra cáp, nguồn PLC và cấu hình IP/route của Windows."
            ))
        })
    };
    let stream = tokio::time::timeout(duration, connection)
        .await
        .map_err(|_| {
            super::diagnostics::note_tcp_failure(endpoint, local_bind_ip);
            crate::plc::diagnostics::event!(
                "PLC_TCP state=TIMEOUT endpoint={endpoint} source_ip={source} routing=system timeout_ms={}",
                duration.as_millis()
            );
            As200Error::Transport(format!(
                "Hết thời gian kết nối TCP PLC {endpoint}, IP nguồn {source}. Kiểm tra cáp, nguồn PLC và cấu hình IP/route của Windows."
            ))
        })??;
    // A new socket is routine for each Reader/prepared ACK. Connection recovery
    // is logged separately by the Worker's observed connection transition.
    if super::diagnostics::take_tcp_failure(endpoint, local_bind_ip) {
        crate::plc::diagnostics::event!(
            "PLC_TCP state=RECONNECTED endpoint={endpoint} source_ip={source} routing=system actual_local={:?} after_failed_open=true",
            stream.local_addr().ok()
        );
    } else if super::diagnostics::debug_enabled() {
        crate::plc::diagnostics::event!(
            "PLC_TCP state=CONNECTED level=DEBUG endpoint={endpoint} source_ip={source} routing=system actual_local={:?}",
            stream.local_addr().ok()
        );
    }
    Ok(stream)
}

#[derive(Debug, Clone)]
pub struct As200Config {
    pub control_start: u16,
    pub record_size: usize,
    pub buffer_count: usize,
    pub address: String,
    pub local_bind_ip: Option<IpAddr>,
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
            Self::Unstable => write!(f, "AS200_SNAPSHOT_UNSTABLE: Control hoặc 240 WORD thay đổi trong khi đọc FC03; bỏ snapshot và đọc lại. Không phát lại ACK chưa rõ kết quả; xem log PLC_CONTROL/PLC_SNAPSHOT để xác định Count, Done hoặc publication thay đổi"),
        }
    }
}
impl std::error::Error for As200Error {}
