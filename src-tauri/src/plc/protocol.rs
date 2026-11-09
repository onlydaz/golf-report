use super::config::PlcConfig;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

pub use super::register_map::{COUNT, START};

async fn read(stream: &mut TcpStream, unit: u8, transaction: u16) -> Result<[u16; COUNT], String> {
    // Chỉ FC03; không có API ghi hoặc điều khiển PLC.
    let [hi, lo] = transaction.to_be_bytes();
    let [ah, al] = START.to_be_bytes();
    stream
        .write_all(&[hi, lo, 0, 0, 0, 6, unit, 3, ah, al, 0, COUNT as u8])
        .await
        .map_err(|e| e.to_string())?;
    let mut header = [0; 7];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|e| e.to_string())?;
    let length = u16::from_be_bytes([header[4], header[5]]) as usize;
    if header[..4] != [hi, lo, 0, 0] || header[6] != unit || !(3..=254).contains(&length) {
        return Err("MBAP không hợp lệ.".into());
    }
    let mut body = vec![0; length - 1];
    stream
        .read_exact(&mut body)
        .await
        .map_err(|e| e.to_string())?;
    if body.len() != 2 + COUNT * 2 || body[0] != 3 || body[1] as usize != COUNT * 2 {
        return Err(format!("PLC trả lỗi hoặc sai số thanh ghi: {body:?}"));
    }
    let mut values = [0; COUNT];
    for (i, bytes) in body[2..].chunks_exact(2).enumerate() {
        values[i] = u16::from_be_bytes([bytes[0], bytes[1]]);
    }
    Ok(values)
}

pub async fn snapshot(config: &PlcConfig) -> Result<Option<[u16; COUNT]>, String> {
    timeout(config.timeout, async {
        let mut stream = TcpStream::connect(&config.address)
            .await
            .map_err(|e| e.to_string())?;
        let first = read(&mut stream, config.unit, 1).await?;
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let second = read(&mut stream, config.unit, 2).await?;
        if first != second {
            return Ok(None);
        }
        Ok(Some(second))
    })
    .await
    .map_err(|_| "Hết thời gian kết nối/đọc PLC.".to_string())?
}
