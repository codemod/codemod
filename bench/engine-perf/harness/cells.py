# (workload, repo, runs, {arm: argv-without-target})
V1="/home/user/eb/bin/v1/node_modules/@codemod.com/cli-linux-x64-gnu/codemod"
V2="/home/user/eb/bin/v2/node_modules/@codemod.com/cli-linux-x64-gnu/codemod"
W="/home/user/eb/bench/workflows"
S1="/home/user/eb/data/src/sma/codemods/solid-migration-assistant"     # c217880, YAML
S2="/home/user/eb/data/src/sma-v2/codemods/solid-migration-assistant"  # 618547c, workflow.ts only
R="/home/user/eb/data/repos"
COMMON=["--no-interactive","--disable-analytics","--allow-fs"]
def yaml(path): return {"v1":[V1,"workflow","run","-w",path]+COMMON, "v2-yaml":[V2,"workflow","run","-w",path]+COMMON}
ENGINE=[]
for wf,repos in [("noop",["probe"]),("shell-chain",["probe"]),
                 ("util-is",["taxonomy","bluebird","excalidraw","calcom"]),
                 ("react-19",["taxonomy","bluebird","excalidraw","calcom"]),
                 ("debarrel",["taxonomy","excalidraw","calcom"]),
                 ("audit",["taxonomy","excalidraw","calcom"])]:
    p = f"{W}/noop.yaml" if wf=="noop" else f"{W}/{wf}/yaml/workflow.yaml"
    for r in repos: ENGINE.append((wf, r, 3 if r=="calcom" else 5, yaml(p)))
SMA=[]
for r in ["tasks-md","codeimage","corvu","kobalte"]:
    n = 3 if r=="kobalte" else 5
    a = yaml(f"{S1}/workflow.yaml"); a["v2-ts"]=[V2,"workflow","run","-w",f"{S2}/workflow.ts","--param","mode=analyze","--param","verify=false","--param","report=none"]+COMMON
    m = yaml(f"{S1}/transform.yaml"); m["v2-ts"]=[V2,"workflow","run","-w",f"{S2}/workflow.ts","--param","mode=migrate","--param","verify=false","--param","report=none","--approve"]+COMMON
    SMA.append(("sma-analyze", r, n, a)); SMA.append(("sma-migrate", r, n, m))
MICRO=[]
for r in ["taxonomy","excalidraw","calcom"]:
    MICRO.append(("jssg-vs-callback", r, 3 if r=="calcom" else 5, {
        "v2-jssg-step":[V2,"workflow","run","-w","/home/user/eb/bench/micro/jssg/workflow.ts"]+COMMON,
        "v2-forEachFile-callback":[V2,"workflow","run","-w","/home/user/eb/bench/micro/each/workflow.ts"]+COMMON}))

# Perf deep dive on Mo's engine (65362215): one arm per local optimization branch.
B="/home/user/eb/bin"
def perf_arms(wf, names):
    y = f"{W}/{wf}/yaml/workflow.yaml"
    a = {}
    for n in names:
        if n == "v1": a[n] = [V1,"workflow","run","-w",y]+COMMON
        elif n == "ts": a[n] = [f"{B}/ts","workflow","run","-w",f"{W}/{wf}/ts/workflow.ts"]+COMMON
        else: a[n] = [f"{B}/mo/codemod" if n=="mo" else f"{B}/{n}","workflow","run","-w",y]+COMMON
    return a
PERF=[]; PAIRS=[]
for wf in ["util-is","react-19","debarrel","audit"]:
    for r in ["taxonomy","excalidraw","calcom"]:
        PERF.append((wf, r, 2 if r=="calcom" else 3,
            perf_arms(wf, ["v1","mo","mo-mem","mo-pool","mo-kinds","mo-rules","mo-stack4","ts"])))
    for r in ["taxonomy","excalidraw"]:
        PAIRS.append((wf, r, 3, perf_arms(wf, ["mo","mo-mem-pool","mo-pool-rules","mo-pool-kinds","mo-kinds-rules","mo-mem-pool-rules","mo-stack4"])))

# Second pass after the restart: audit on cal.com, the pure single changes, the step cap, and five-way stack.
AUDITCAL=[("audit","calcom",2,perf_arms("audit",["v1","mo","mo-mem","mo-pool","mo-kinds","mo-rules","mo-stack4","ts","mo-cap-pure","mo-stack5-pure"]))]
SINGLES=[]
for wf in ["util-is","react-19","debarrel","audit"]:
    for r in ["taxonomy","excalidraw"]:
        SINGLES.append((wf, r, 3, perf_arms(wf, ["mo","mo-kinds-pure","mo-rules-pure","mo-cap-pure","mo-stack4","mo-stack5-pure"])))
