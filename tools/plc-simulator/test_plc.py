"""Client chỉ đọc Holding Registers; không ghi PLC và không truy cập PostgreSQL."""

from pymodbus.client import ModbusTcpClient
from pymodbus.exceptions import ModbusException

HOST, PORT, UNIT_ID = "127.0.0.1", 1502, 1
START_ADDRESS, REGISTER_COUNT = 100, 18
RESULT_LABELS = {1: "OK", 2: "NG", 3: "BỎ QUA"}


def display_registers(registers: list[int]) -> None:
    """Dùng chung phần hiển thị cho server và client."""
    if len(registers) != REGISTER_COUNT:
        raise ValueError("Cần đúng 18 Holding Registers.")
    print("\nThanh ghi thô (địa chỉ: giá trị):")
    print(" | ".join(f"{START_ADDRESS + i}: {value}" for i, value in enumerate(registers)))
    print(f"completed={registers[0]}, sequence_id={registers[1]}")
    if registers[0] != 1:
        print("Dữ liệu chưa hoàn thành. Hãy tạo phép đo ở Terminal Simulator rồi đọc lại.", flush=True)
        return
    print(f"Số gậy: {registers[2]} | Weight: {registers[3] / 100:.2f}")
    for i in range(4):
        result = registers[12 + i]
        distance = f"{registers[4 + i] / 100:.2f} mm" if result != 3 else "— (thô: 0)"
        print(f"Tip {i + 1}: {distance} | Kiểm tra lại: {registers[8 + i]} | "
              f"{RESULT_LABELS.get(result, f'KHÔNG HỢP LỆ ({result})')}")
    print(f"Butt: {registers[16] / 100:.2f} mm | {RESULT_LABELS.get(registers[17], 'KHÔNG HỢP LỆ')}")
    overall = "NG" if 2 in registers[12:16] or registers[17] == 2 else (
        "OK" if registers[12:16] == [1] * 4 and registers[17] == 1 else "KHÔNG HỢP LỆ"
    )
    print(f"Kết quả tổng: {overall}", flush=True)


def read_measurement(client: ModbusTcpClient) -> list[int] | None:
    """Đọc đủ 18 thanh ghi, kiểm tra completed và sequence để tránh dữ liệu đang cập nhật."""
    response = client.read_holding_registers(START_ADDRESS, count=REGISTER_COUNT, device_id=UNIT_ID)
    if response.isError():
        raise ValueError(f"PLC trả về lỗi Modbus: {response}")
    registers = response.registers
    if len(registers) != REGISTER_COUNT:
        raise ValueError(f"PLC chỉ trả về {len(registers)}/18 thanh ghi.")
    if registers[0] == 1:
        confirmation = client.read_holding_registers(START_ADDRESS, count=2, device_id=UNIT_ID)
        if confirmation.isError():
            raise ValueError(f"Không xác nhận được dữ liệu: {confirmation}")
        if confirmation.registers != registers[:2]:
            print("Dữ liệu đang cập nhật. Nhấn Enter để đọc lại.")
            return None
    return registers


def main() -> None:
    client = ModbusTcpClient(HOST, port=PORT, timeout=3, retries=0)
    try:
        while True:
            try:
                if not client.connect():
                    print(f"Không kết nối được {HOST}:{PORT}. Hãy khởi động PLC Simulator.")
                else:
                    registers = read_measurement(client)
                    if registers is not None:
                        display_registers(registers)
            except (ModbusException, OSError, ValueError) as error:
                print(f"Không đọc được PLC: {error}")
                client.close()
            if input("\nEnter: đọc lại | 0: thoát: ").strip() == "0":
                break
    except (KeyboardInterrupt, EOFError):
        print("\nDừng đọc PLC.")
    finally:
        client.close()


if __name__ == "__main__":
    main()
