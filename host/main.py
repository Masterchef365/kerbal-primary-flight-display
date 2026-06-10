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

while True:
    try:
        vessel = conn.space_center.active_vessel
    except:
        print("Waiting for vessel...")
        time.sleep(1)
        continue
    break

flight_info = vessel.flight()
flight_info.velocity
altitude = conn.add_stream(getattr, flight_info, 'mean_altitude')
pitch = conn.add_stream(getattr, flight_info, 'pitch')
roll = conn.add_stream(getattr, flight_info, 'roll')
heading = conn.add_stream(getattr, flight_info, 'heading')
true_air_speed = conn.add_stream(getattr, flight_info, 'true_air_speed')
orbital_speed = conn.add_stream(
    getattr,
    vessel.orbit,
    'speed'
)

while True:
    obt_speed = orbital_speed()
    air_speed = true_air_speed()

    true_speed = -1.0
    if air_speed > 0.0:
        true_speed = air_speed
    else:
        true_speed = obt_speed

    bytes = struct.pack(
        '4B5f',
        0x00,
        0x00,
        0x00,
        0xff,
        pitch(),
        roll(),
        altitude(),
        true_speed,
        heading()
    )

    now = time.time()
    ser.write(bytes)

    try:
        print(ser.read_all().decode('utf-8'))
    except:
        pass
    #print("TIME: ", time.time() - now)

    time.sleep(0.1)

