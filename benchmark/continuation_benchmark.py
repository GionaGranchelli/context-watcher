#!/usr/bin/env python3
"""Small live OMP continuation qualification; no latency or general quality claim.

Uses an isolated agent directory and workspace. Credentials are read from a named
variable, never command arguments. Saves native sessions, exports and new answers.
"""
import argparse
import ast
import sys
import tempfile
import hashlib
import json
import os
from pathlib import Path
import queue
import re
import shutil
import subprocess
import threading
import time

DIAGNOSTICS = """Do not implement it yet. First use bash exactly once to run:
python3 -c "print('\\n'.join('diagnostic row %04d: synthetic successful fixture event' % i for i in range(120)))"
Then reply READY. Do not delegate or use other tools."""
CONTINUE = """Now implement the function from the earlier task contract. Reply with
only a JSON object whose 'code' field is the complete Python module source. Do
not use tools. Preserve every requirement from the original contract."""

TASKS = {
    "bucket": {
        "contract": "Implement Python function bucket(n): reject bool and non-int with TypeError; reject negative integers with ValueError; return 'copper' for 0..6, 'silver' for 7..22, and 'jade' for 23 and above.",
        "function": "bucket",
        "names": {"n", "bool", "int", "TypeError", "ValueError", "isinstance", "type"},
        "calls": {"isinstance", "type", "TypeError", "ValueError"},
        "methods": set(),
        "extra_ast": (),
        "assertions": 12,
        "checks": '''for value,expected in [(0,"copper"),(6,"copper"),(7,"silver"),(22,"silver"),(23,"jade"),(999999,"jade")]:
    assert function(value)==expected,(value,expected)
for value,error in [(-1,ValueError),(True,TypeError),(False,TypeError),(1.0,TypeError),("7",TypeError),(None,TypeError)]:
    try: function(value)
    except error: pass
    else: raise AssertionError(f"Expected {error.__name__} for {value!r}")''',
    },
    "normalize-tags": {
        "contract": "Implement Python function normalize_tags(values): require a list of strings or raise TypeError; strip whitespace and lowercase each string; omit empty strings; remove duplicates while preserving the first occurrence order; return a new list.",
        "function": "normalize_tags",
        "names": {"values", "value", "result", "seen", "tag", "list", "str", "TypeError", "isinstance", "set"},
        "calls": {"isinstance", "TypeError", "set"},
        "methods": {"strip", "lower", "casefold", "add", "append"},
        "extra_ast": (ast.For, ast.Assign, ast.List, ast.Set, ast.Store, ast.Attribute, ast.NotIn, ast.In, ast.Expr, ast.Continue),
        "assertions": 12,
        "checks": '''cases=[
    ([" Alpha ","BETA","alpha","","  ","Z"],["alpha","beta","z"]),
    ([],[]),
    (["  "],[]),
    (["A","b","a"],["a","b"]),
    (["x","y","x","z","Y"],["x","y","z"]),
    (["Z","a"],["z","a"]),
]
for values,expected in cases: assert function(values)==expected,(values,expected)
for values in ["a",("a",),None,[1],[None],[True]]:
    try: function(values)
    except TypeError: pass
    else: raise AssertionError(f"Expected TypeError for {values!r}")''',
    },
    "transition": {
        "contract": "Implement Python function transition(state, event). Both arguments must be strings or raise TypeError. Valid transitions are queued+start -> running; running+complete -> done; running+fail -> failed; failed+retry -> queued. Every other state/event pair raises ValueError.",
        "function": "transition",
        "names": {"state", "event", "str", "TypeError", "ValueError", "isinstance", "KeyError"},
        "calls": {"isinstance", "TypeError", "ValueError"},
        "methods": {"get"},
        "extra_ast": (ast.Assign, ast.Dict, ast.Tuple, ast.Subscript, ast.Try, ast.ExceptHandler, ast.JoinedStr, ast.FormattedValue, ast.Set, ast.In, ast.NotIn),
        "states": {"queued", "running", "done", "failed"},
        "transition_table": {
            ("queued", "start"): "running",
            ("running", "complete"): "done",
            ("running", "fail"): "failed",
            ("failed", "retry"): "queued",
        },
        "assertions": 12,
        "checks": '''for state,event,expected in [("queued","start","running"),("running","complete","done"),("running","fail","failed"),("failed","retry","queued")]:
    assert function(state,event)==expected,(state,event,expected)
for state,event in [("done","retry"),("queued","complete"),("running","retry"),("failed","start"),("unknown","start")]:
    try: function(state,event)
    except ValueError: pass
    else: raise AssertionError(f"Expected ValueError for {(state,event)!r}")
for state,event in [(None,"start"),("queued",1),(1,2)]:
    try: function(state,event)
    except TypeError: pass
    else: raise AssertionError(f"Expected TypeError for {(state,event)!r}")''',
    },
}


def run(args, env, cwd, log, timeout=120):
    with log.open("w") as output:
        result = subprocess.run(args, env=env, cwd=cwd, stdout=output,
                                stderr=subprocess.STDOUT, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"Command failed ({result.returncode}); see {log}")
    return log.read_text()


def compact(args, env, cwd, log):
    """Wait for the actual native compact response, not a slash-command echo."""
    events = queue.Queue()
    with log.open("w") as output:
        proc = subprocess.Popen(args + ["--mode", "rpc", "--no-ui"], env=env, cwd=cwd,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=output, text=True)
        def reader():
            for line in proc.stdout:
                output.write(line); output.flush()
                try: events.put(json.loads(line))
                except json.JSONDecodeError: pass
        thread = threading.Thread(target=reader, daemon=True); thread.start()
        deadline = time.monotonic() + 240
        try:
            while time.monotonic() < deadline:
                event = events.get(timeout=max(.1, deadline-time.monotonic()))
                if event.get("type") == "ready":
                    proc.stdin.write(json.dumps({"id":"native", "type":"compact"})+"\n"); proc.stdin.flush()
                if event.get("type") == "response" and event.get("id") == "native":
                    if not event.get("success"): raise RuntimeError(f"Native compaction failed; see {log}")
                    return event
            raise TimeoutError("Native compaction deadline")
        finally:
            proc.stdin.close()
            try: proc.wait(timeout=5)
            except subprocess.TimeoutExpired: proc.terminate(); proc.wait(timeout=5)
            thread.join(timeout=2)


def safe_attribute_receiver(node, task, local_names, global_names):
    if isinstance(node, ast.Name):
        return node.id in task["names"] or node.id in local_names or node.id in global_names
    return (
        isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and node.func.attr in task["methods"]
        and safe_attribute_receiver(node.func.value, task, local_names, global_names)
    )


def check_answer(text, task):
    # Compile the newly generated answer; no preexisting artifact can pass this gate.
    decoder = json.JSONDecoder()
    for match in re.finditer(r'\{', text):
        try: answer, _ = decoder.raw_decode(text[match.start():])
        except json.JSONDecodeError: continue
        if isinstance(answer, dict) and isinstance(answer.get("code"), str): break
    else: raise AssertionError("No JSON code answer")
    # Reject imports, unapproved attribute calls and arbitrary code before execution.
    tree=ast.parse(answer["code"])
    allowed=(ast.Module,ast.FunctionDef,ast.arguments,ast.arg,ast.If,ast.Return,ast.Raise,
             ast.Expr,ast.Constant,ast.Name,ast.Load,ast.Store,ast.Call,ast.Compare,ast.BoolOp,
             ast.UnaryOp,ast.Not,ast.And,ast.Or,ast.Eq,ast.NotEq,ast.Lt,ast.LtE,ast.Gt,
             ast.GtE,ast.Is,ast.IsNot,ast.USub,ast.UAdd)+task["extra_ast"]
    functions=[node for node in tree.body if isinstance(node,ast.FunctionDef)]
    assert len(functions)==1 and functions[0].name==task["function"]
    function_node=functions[0]
    module_assignments=[node for node in tree.body if node is not function_node]
    global_constants={}
    for assignment in module_assignments:
        assert isinstance(assignment,ast.Assign) and len(assignment.targets)==1
        assert isinstance(assignment.targets[0],ast.Name)
        name=assignment.targets[0].id
        if isinstance(assignment.value,ast.Set):
            assert all(isinstance(item,ast.Constant) and isinstance(item.value,str) for item in assignment.value.elts)
            value=set(item.value for item in assignment.value.elts)
            assert value==task.get("states")
            role="states"
        else:
            assert isinstance(assignment.value,ast.Dict)
            value={}
            for key,item in zip(assignment.value.keys,assignment.value.values):
                assert isinstance(key,ast.Tuple) and len(key.elts)==2
                assert all(isinstance(part,ast.Constant) and isinstance(part.value,str) for part in key.elts)
                assert isinstance(item,ast.Constant) and isinstance(item.value,str)
                value[tuple(part.value for part in key.elts)]=item.value
            assert value==task.get("transition_table")
            role="transitions"
        assert role not in global_constants.values()
        global_constants[name]=role
    if task.get("states"):
        assert set(global_constants.values()).issubset({"states", "transitions"})
    else:
        assert len(tree.body)==1
    assert not function_node.decorator_list
    local_names={arg.arg for arg in function_node.args.args}
    local_names.update(node.id for node in ast.walk(function_node) if isinstance(node,ast.Name) and isinstance(node.ctx,ast.Store))
    global_names=set(global_constants)
    transition_names={name for name,role in global_constants.items() if role=="transitions"}
    for node in ast.walk(tree):
        assert isinstance(node,allowed), f"Unsupported generated syntax: {type(node).__name__}"
        if isinstance(node,ast.Name): assert node.id in task["names"] or node.id in local_names or node.id in global_names, f"Unapproved name: {node.id}"
        if isinstance(node,ast.Subscript):
            assert isinstance(node.ctx,ast.Load) and isinstance(node.value,ast.Name) and node.value.id in transition_names
            assert isinstance(node.slice,ast.Tuple) and [item.id for item in node.slice.elts if isinstance(item,ast.Name)]==["state","event"]
        if isinstance(node,ast.ExceptHandler): assert isinstance(node.type,ast.Name) and node.type.id in {"KeyError"}
        if isinstance(node,ast.Call):
            if isinstance(node.func,ast.Name): assert node.func.id in task["calls"]
            else:
                assert isinstance(node.func,ast.Attribute) and node.func.attr in task["methods"]
                assert safe_attribute_receiver(node.func.value,task,local_names,global_names)
    checker = r'''import json, resource, sys
resource.setrlimit(resource.RLIMIT_CPU,(2,2))
resource.setrlimit(resource.RLIMIT_AS,(134217728,134217728))
scope={}
exec(compile(json.load(sys.stdin),"generated_answer.py","exec"),scope)
function=scope["''' + task["function"] + r'''"]
''' + task["checks"] + "\n"
    with tempfile.TemporaryDirectory(prefix="cw-answer-check-") as cwd:
        subprocess.run([sys.executable,"-I","-S","-c",checker], input=json.dumps(answer["code"]),
                       text=True,check=True,timeout=5,env={},cwd=cwd,capture_output=True)
    return answer["code"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cw", required=True, type=Path)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--task", required=True, choices=sorted(TASKS))
    parser.add_argument("--key-env", default="CW_BENCHMARK_API_KEY")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--seed", type=Path, help="Reuse an owned, stopped native seed instead of calling the model again")
    args = parser.parse_args()
    task = TASKS[args.task]
    root = args.output.resolve(); root.mkdir(mode=0o700, parents=True, exist_ok=False)
    agent = root / "agent"; agent.mkdir(mode=0o700)
    workspace = root / "workspace"; workspace.mkdir()
    env = os.environ.copy(); env["PI_CODING_AGENT_DIR"] = str(agent)
    if not env.get(args.key_env): raise ValueError("Missing credential environment variable")
    # Explicit local route: never use an ambient HTTP compression proxy or fallback.
    for name in ["HTTP_PROXY","HTTPS_PROXY","ALL_PROXY","http_proxy","https_proxy","all_proxy"]: env.pop(name,None)
    models = {"providers":{"cw-qualification":{"baseUrl":args.base_url,"api":"openai-completions","apiKey":args.key_env,"authHeader":True,"models":[{"id":args.model,"name":"Qualification model","reasoning":True,"input":["text"],"contextWindow":65536,"maxTokens":2048,"compat":{"thinkingFormat":"qwen-chat-template","supportsDeveloperRole":False}}]}}}
    # JSON is valid YAML; the extension avoids legacy migration behavior.
    (agent / "models.yml").write_text(json.dumps(models))
    common = ["omp","--model",f"cw-qualification/{args.model}","--thinking=off","--no-extensions","--no-skills","--no-rules","--no-title","--no-lsp","--no-pty","--max-time","90"]
    session_dir = root / "seed-sessions"; session_dir.mkdir()
    if args.seed:
        shutil.copyfile(args.seed, session_dir / "seed.jsonl")
    else:
        contract = "Remember this task contract for later. " + task["contract"] + "\n" + DIAGNOSTICS
        run(common+["--tools","bash","--approval-mode","yolo","--session-dir",str(session_dir),"-p",contract],env,workspace,root/"seed.log")
    sessions=list(session_dir.rglob("*.jsonl"))
    if len(sessions)!=1: raise RuntimeError(f"Expected one native seed session, found {len(sessions)}")
    seed=sessions[0]
    if not args.seed:
        run(common+["--no-tools","--resume",str(seed),"-p","The diagnostics are complete. Acknowledge without implementing the task yet."],env,workspace,root/"boundary.log")
    source_hash=hashlib.sha256(seed.read_bytes()).hexdigest()
    results={"scope":"one synthetic task, one trial per arm; not general quality or performance qualification","task":args.task,"model":args.model,"harness":subprocess.check_output(["omp","--version"],text=True).strip(),"source_sha256":source_hash,"binary_sha256":hashlib.sha256(args.cw.read_bytes()).hexdigest(),"runner_sha256":hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),"arms":{}}
    for arm in ["baseline","min","mid","max","native"]:
        folder=root/arm; folder.mkdir(); session=folder/"session.jsonl"; shutil.copyfile(seed,session)
        result={"status":"failed"}
        try:
            if arm in ["min","mid","max"]:
                db=folder/"cw.db"
                out=run([str(args.cw),"--db",str(db),"import",str(session),"--harness","omp"],env,workspace,folder/"import.log")
                sid=out.split("session:")[-1].strip().splitlines()[0]
                run([str(args.cw),"--db",str(db),"optimize","--session",sid,"--strategy",arm,"--sync-back"],env,workspace,folder/"optimize.log")
                exports=list(folder.glob("*.cw-export-*.jsonl"))
                if len(exports)!=1: raise RuntimeError("Expected one exported transcript")
                result["source_unchanged"]=session.read_bytes()==seed.read_bytes()
                result["export_changed"]=exports[0].read_bytes()!=session.read_bytes()
                if arm in ("mid","max") and not result["export_changed"]:
                    raise AssertionError("Expected eligible log reduction; export was unchanged")
                if not result["source_unchanged"]: raise AssertionError("Source changed")
                session=exports[0]
                result["preview"]=json.loads(run([str(args.cw),"--db",str(db),"preview","--session",sid],env,workspace,folder/"preview.json"))["turn"]["token_usage"]
            elif arm=="native":
                overlay = folder / "native.yml"
                overlay.write_text("compaction:\n  keepRecentTokens: 256\n  reserveTokens: 2048\n")
                result["native_settings"]={"keepRecentTokens":256,"reserveTokens":2048}
                result["native_compaction"]=compact(common+["--no-tools","--config",str(overlay),"--resume",str(session)],env,workspace,folder/"compact.jsonl")
            result["continuation_source_sha256"]=hashlib.sha256(session.read_bytes()).hexdigest()
            text=run(common+["--no-tools","--resume",str(session),"-p",CONTINUE],env,workspace,folder/"answer.log")
            code=check_answer(text,task); (folder/"generated_answer.py").write_text(code)
            result.update(status="passed",independent_assertions=task["assertions"])
        except Exception as error: result["error"]=str(error)
        results["arms"][arm]=result
        (root/"results.json").write_text(json.dumps(results,indent=2))
        print(arm, result["status"], flush=True)
    assert hashlib.sha256(seed.read_bytes()).hexdigest()==source_hash
    print(root/"results.json")
    if any(r["status"]!="passed" for r in results["arms"].values()): raise SystemExit(1)

if __name__=="__main__": main()
