import sys,pathlib
assert pathlib.Path(sys.argv[1]).name == "alia-server", sys.argv
print("Cargo selected:", sys.argv[1])
print("Runner deliberately did not execute the server.")
