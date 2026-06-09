import krpc
import time
import serial
import json

conn = krpc.connect(
    name='RustPFD',
    address='192.168.50.217',
    rpc_port=50000, 
    stream_port=50001
)

ser = serial.Serial("/dev/ttyACM0", baudrate=921600, timeout=0.1)

vessel = conn.space_center.active_vessel
flight_info = vessel.flight()
altitude = conn.add_stream(getattr, flight_info, 'mean_altitude')
pitch = conn.add_stream(getattr, flight_info, 'pitch')
roll = conn.add_stream(getattr, flight_info, 'roll')
speed = conn.add_stream(getattr, flight_info, 'speed')
heading = conn.add_stream(getattr, flight_info, 'heading')

while True:
    data = {
        "pitch": pitch(),
        "roll": roll(),
        "altitude": altitude(),
        "speed": speed(),
        "heading": heading(),
    }

    now = time.time()
    ser.write(json.dumps(data).encode('utf-8'))
    ser.write(b'\n')

    try:
        print(ser.read_all().decode('utf-8'))
    except:
        pass
    print("TIME: ", time.time() - now)

    #time.sleep(0.01)

