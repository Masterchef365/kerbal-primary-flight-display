import krpc
import time
import serial
import json
import struct

conn = krpc.connect(
    name='RustPFD',
    address='192.168.50.217',
    rpc_port=50000, 
    stream_port=50001
)

ser = serial.Serial("/dev/ttyACM0", baudrate=115200)

vessel = conn.space_center.active_vessel
flight_info = vessel.flight()
altitude = conn.add_stream(getattr, flight_info, 'mean_altitude')
pitch = conn.add_stream(getattr, flight_info, 'pitch')
roll = conn.add_stream(getattr, flight_info, 'roll')
speed = conn.add_stream(getattr, flight_info, 'speed')
heading = conn.add_stream(getattr, flight_info, 'heading')

while True:
    bytes = struct.pack(
        '4B5f',
        0x00,
        0x00,
        0x00,
        0xff,
        pitch(),
        roll(),
        altitude(),
        speed(),
        heading()
    )

    now = time.time()
    ser.write(bytes)

    try:
        print(ser.read_all().decode('utf-8'))
    except:
        pass
    print("TIME: ", time.time() - now)

    time.sleep(0.1)

