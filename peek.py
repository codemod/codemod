import json,sys
lines=open(sys.argv[1]).read().splitlines()
n=int(sys.argv[2]) if len(sys.argv)>2 else 6
tools=0
for l in lines:
    try: d=json.loads(l)
    except: continue
    if d.get("type")=="assistant":
        tools+=sum(1 for c in d["message"]["content"] if c["type"]=="tool_use")
print("lines",len(lines),"tool calls",tools)
for l in lines[-n:]:
    try: d=json.loads(l)
    except: continue
    if d.get("type")=="assistant":
        for c in d["message"]["content"]:
            if c["type"]=="tool_use": print("tool:",c["name"],str(c["input"])[:160])
            elif c["type"]=="text": print("text:",c["text"][:160])
    elif d.get("type")=="user":
        cs=d["message"].get("content")
        for c in cs if isinstance(cs,list) else []:
            if c.get("type")=="tool_result": print("result:",str(c.get("content"))[:160])
    else: print(d.get("type"), d.get("subtype"), str(d.get("permission_denials",""))[:200])
