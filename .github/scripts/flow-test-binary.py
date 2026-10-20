import json, sys
executables = []
for line in open(sys.argv[1]):
    item = json.loads(line)
    if item.get("reason") == "compiler-artifact" and item.get("target", {}).get("name") == "xenterm" and item.get("profile", {}).get("test") and item.get("executable"):
        executables.append(item["executable"])
assert len(executables) == 1, executables
print(executables[0])
