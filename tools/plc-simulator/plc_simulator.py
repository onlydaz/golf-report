"""PLC giả lập với register map riêng, không truy cập ứng dụng hay PostgreSQL."""

import asyncio
import logging
import queue
import random
import threading

from pymodbus.datastore import (
    ModbusDeviceContext,
    ModbusSequentialDataBlock,
    ModbusServerContext,
)
from pymodbus.server import ModbusTcpServer

from test_plc import display_registers

HOST, PORT, UNIT_ID = "127.0.0.1", 1502, 1
START_ADDRESS, REGISTER_COUNT = 100, 18
OK, NG, SKIPPED = 1, 2, 3


def generate_measurement(mode: str, sequence_id: int) -> list[int]:
    """Tất cả giá trị truyền là unsigned 16-bit; khoảng cách/weight nhân 100."""
    if mode not in ("1", "2", "3"):
        raise ValueError("Chế độ phải là 1, 2 hoặc 3.")
    distances = [random.randint(900, 1500) for _ in range(4)]
    retries = [random.randint(0, 4) for _ in range(4)]
    results = [OK] * 4
    if mode == "2":
        # Chọn một trong ba vị trí đầu để luôn có vị trí phía sau BỎ QUA.
        failed = random.randrange(3)
        results[failed], retries[failed] = NG, 4
        for index in range(failed + 1, 4):
            distances[index], retries[index], results[index] = 0, 0, SKIPPED
    return [
        1, sequence_id, sequence_id, random.randint(9000, 13000),
        *distances, *retries, *results,
        random.randint(900, 1500), NG if mode == "3" else OK,
    ]


def terminal_menu(commands: queue.Queue) -> None:
    """Input chạy ở daemon thread để không chặn server và không giữ tiến trình khi thoát."""
    while True:
        print("\n1: Tổng OK | 2: Tip NG | 3: Butt NG | 0: Thoát", flush=True)
        try:
            choice = input("Chọn: ").strip()
        except (EOFError, KeyboardInterrupt):
            choice = "0"
        commands.put(choice)
        if choice == "0":
            return


async def main() -> None:
    # ModbusDeviceContext cộng 1 nội bộ: địa chỉ PDU 100 tương ứng block 101.
    device = ModbusDeviceContext(hr=ModbusSequentialDataBlock(101, [0] * REGISTER_COUNT))
    context = ModbusServerContext(devices={UNIT_ID: device}, single=False)
    server = ModbusTcpServer(context, address=(HOST, PORT))
    commands = queue.Queue()
    try:
        await server.serve_forever(background=True)
        print(f"PLC Simulator: {HOST}:{PORT}, Unit ID={UNIT_ID}, HR 100–117", flush=True)
        threading.Thread(target=terminal_menu, args=(commands,), daemon=True).start()
        sequence_id = 0
        while True:
            try:
                choice = commands.get_nowait()
            except queue.Empty:
                await asyncio.sleep(0.05)
                continue
            if choice == "0":
                break
            if choice not in ("1", "2", "3"):
                print("Lựa chọn không hợp lệ. Nhập 0, 1, 2 hoặc 3.", flush=True)
                continue
            # Thanh ghi 16-bit: quay vòng về 1 sau 65535; 0 dành cho khởi động.
            sequence_id = sequence_id % 65535 + 1
            registers = generate_measurement(choice, sequence_id)
            device.setValues(3, START_ADDRESS, [0])
            device.setValues(3, START_ADDRESS + 1, registers[1:])
            device.setValues(3, START_ADDRESS, [1])
            display_registers(registers)
    finally:
        await server.shutdown()
        print("Đã đóng PLC Simulator.", flush=True)


if __name__ == "__main__":
    logging.basicConfig(level=logging.WARNING)
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("Đã dừng bằng Ctrl+C.")
    except (OSError, RuntimeError) as error:
        print(f"Không thể chạy server tại {HOST}:{PORT}: {error}")
        raise SystemExit(1)
