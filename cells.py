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
