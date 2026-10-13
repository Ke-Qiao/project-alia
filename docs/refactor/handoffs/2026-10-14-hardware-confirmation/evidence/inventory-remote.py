import json, subprocess, datetime
from pathlib import Path
checks={
 "kernel":["uname","-a"],"os":["cat","/etc/os-release"],
 "architecture":["uname","-m"],"memory":["free","-h"],"disk":["df","-h","/"],
 "account":["id"],"alsa_cards":["cat","/proc/asound/cards"],
 "capture_devices":["arecord","-l"],"playback_devices":["aplay","-l"],
 "capture_pcms":["arecord","-L"],"playback_pcms":["aplay","-L"],
 "usb":["lsusb"],"serial_ids":["ls","-l","/dev/serial/by-id","/dev/serial/by-path"],
 "rust":["rustc","-Vv"],"cargo":["cargo","-V"],"protoc":["protoc","--version"]}
result={"recorded_at":datetime.datetime.now(datetime.timezone.utc).isoformat(),"mode":"read-only inventory; no device open, recording, playback, reset or writes", "checks":{}}
for name,cmd in checks.items():
 try:
  p=subprocess.run(cmd,capture_output=True,text=True,timeout=3)
  result["checks"][name]={"command":cmd,"exit_code":p.returncode,"stdout":p.stdout,"stderr":p.stderr}
 except (OSError,subprocess.TimeoutExpired) as e:
  result["checks"][name]={"command":cmd,"error":type(e).__name__}
for name,path in [("board_model","/proc/device-tree/model"),("thermal_millidegrees","/sys/class/thermal/thermal_zone0/temp")]:
 try:result[name]=Path(path).read_text().strip("\x00\n")
 except OSError as e:result[name]={"error":type(e).__name__}
result["i2c_nodes"]=[str(p) for p in Path("/dev").glob("i2c-*")]
print(json.dumps(result,ensure_ascii=False,indent=2))
