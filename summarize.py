import json,glob,os,statistics as st
rows=[]
for d in sorted(glob.glob("/mnt/project-files/agent-vs-sma/runs/*")):
    r={"run":os.path.basename(d)}
    if not os.path.exists(d+"/status"): r["state"]="running"; rows.append(r); continue
    res=[json.loads(l) for l in open(d+"/transcript.jsonl") if '"type":"result"' in l]
    last=res[-1] if res else {}
    r["wall_min"]=round(int(open(d+"/wall-seconds").read())/60,1)
    r["cost"]=round(last.get("total_cost_usd",0),2)
    r["budget_hit"]=any("budget" in str(x.get("subtype","")) for x in res)
    tools=0; tsc=0
    for l in open(d+"/transcript.jsonl"):
        try: x=json.loads(l)
        except: continue
        if x.get("type")=="assistant":
            for c in x["message"]["content"]:
                if c["type"]=="tool_use":
                    tools+=1
                    if c["name"]=="Bash" and "tsc" in str(c["input"].get("command","")): tsc+=1
    r["tool_calls"]=tools; r["tsc_runs"]=tsc
    g=json.load(open(d+"/grade.json")) if os.path.getsize(d+"/grade.json") else {}
    r["solid2"]=g.get("all_solid_2"); r["tsc"]=g.get("tsc"); r["finished"]=g.get("finished"); r["diff"]=g.get("diff")
    rows.append(r)
for r in rows: print(r)
