"""Local qualification: real Nickel/OpenTofu and a disposable HTTP API fixture."""
import copy
import http.server
import json
import os
from pathlib import Path
import subprocess
import signal
import time
import tempfile
import threading
import urllib.parse

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ["PLANE_BIN"])
TF_BINARY = Path(os.environ["TF_BIN"])
TOFU = os.environ["TOFU_BIN"]

class API(http.server.BaseHTTPRequestHandler):
    objects = {}
    writes = 0
    history = []
    def log_message(self, *args):
        pass
    def reply(self, code, body):
        self.history.append((self.command,self.path,code))
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)
    def do_GET(self):
        route = urllib.parse.urlsplit(self.path)
        if "watch=true" in route.query:
            self.reply(410, {})
        else:
            self.reply(200 if route.path in self.objects else 404, self.objects.get(route.path, {}))
    def mutation(self):
        route = urllib.parse.urlsplit(self.path)
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.command == "DELETE":
            old = self.objects.get(route.path)
            if old is None:
                return self.reply(404, {})
            assert body["preconditions"]["uid"] == old["metadata"]["uid"]
            del self.objects[route.path]
            API.writes += 1
            return self.reply(200, {"status":"Success"})
        path = route.path + "/" + body["metadata"]["name"] if self.command == "POST" else route.path
        old = self.objects.get(path)
        if self.command == "POST" and old:
            return self.reply(409, {})
        if self.command == "PATCH":
            assert self.headers["Content-Type"] == "application/apply-patch+yaml"
            if old is None or body["metadata"]["resourceVersion"] != old["metadata"]["resourceVersion"]:
                return self.reply(409, {})
        if "dryRun=All" not in route.query:
            API.writes += 1
            body["metadata"]["uid"] = old["metadata"]["uid"] if old else "uid-" + str(API.writes)
            body["metadata"]["resourceVersion"] = str(API.writes)
            self.objects[path] = copy.deepcopy(body)
        self.reply(200, body)
    do_POST = mutation
    do_PATCH = mutation
    do_DELETE = mutation


def invoke(*args, good=True):
    print("RUN", args[0],flush=True)
    result = subprocess.run([str(BINARY), *map(str,args)], text=True, capture_output=True, timeout=180)
    if good:
        assert result.returncode == 0, (args, result.stdout, result.stderr,API.history[-12:])
        return json.loads(result.stdout)
    assert result.returncode != 0, args
    return result


def main():
    with tempfile.TemporaryDirectory(prefix="plane-e2e-") as tmp:
        tmp = Path(tmp)
        bundle = tmp / "bundle"
        subprocess.run([str(TF_BINARY), "import-schema", "--schema", str(ROOT.parent/"nickel-tf/examples/local/schema.json"), "--selection", str(ROOT.parent/"nickel-tf/examples/local/selection.json"), "--output", str(bundle)], check=True, timeout=30)
        source = tmp / "main.ncl"
        source.write_text((ROOT/"examples/mixed/main.ncl").read_text().replace('mode="observe"','mode="manage",deletion="delete",allowDelete=true'))
        targets = tmp/"targets.json"
        catalog={"version":1,"targets":{"local":{"backend":"opentofu","config":{"binary":TOFU,"stack":"fixture"}},"cluster":{"backend":"kubernetes","config":{"server":"http://127.0.0.1:1","namespace":"demo","allowHttpLoopback":True}}}}
        targets.write_text(json.dumps(catalog))
        args=["--config",source,"--resources",ROOT,"--targets",targets,"--tf-resources",ROOT.parent/"nickel-tf","--k8s-resources",ROOT.parent/"nickel-k8s","--schema",f"local={bundle}"]
        checked=invoke("check",*args)
        rendered=invoke("render",*args)
        assert checked["revision"]==rendered["revision"]
        lock=tmp/"lock.json";lock.write_text(json.dumps(invoke("lock",*args)))
        assert "hello from" not in lock.read_text()
        invoke("check",*args,"--locked",lock)
        server=http.server.ThreadingHTTPServer(("127.0.0.1",0),API)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:
            catalog["targets"]["cluster"]["config"]["server"]=f"http://127.0.0.1:{server.server_port}"
            targets.write_text(json.dumps(catalog));store=tmp/"store"
            plan=invoke("plan",*args,"--store",store)
            assert API.writes==0
            operation=plan["units"]["infrastructure"]["change"]["operation"]
            invoke("apply",*args,"--store",store,"--plan",operation)
            state=invoke("reconcile",*args,"--store",store,"--once")
            assert all(u["conditions"]["Applied"] for u in state["units"].values()), state
            assert not state["units"]["infrastructure"]["conditions"]["Ready"]
            obj=next(iter(API.objects.values()))
            assert obj["data"]["message"]=="hello from OpenTofu"
            writes=API.writes
            invoke("reconcile",*args,"--store",store,"--once")
            assert API.writes==writes
            invoke("apply",*args,"--store",store,"--plan",operation,good=False)
            source.write_text(source.read_text().replace("hello from OpenTofu","updated"))
            invoke("check",*args,"--locked",lock,good=False)
            changed=invoke("reconcile",*args,"--store",store,"--once")
            assert all(u["conditions"]["Applied"] for u in changed["units"].values()),changed
            assert next(iter(API.objects.values()))["data"]["message"]=="updated"
            with (tmp/"serve.log").open("w") as log:
                service=subprocess.Popen([str(BINARY),"serve",*map(str,args),"--store",str(store),"--interval-seconds","1"],stdout=log,stderr=log)
                try:
                    time.sleep(0.5)
                    invoke("status","--store",store,"--instance","mixed-demo")
                    invoke("reconcile",*args,"--store",store,"--once",good=False)
                    service.send_signal(signal.SIGTERM)
                    assert service.wait(timeout=180)==0,(tmp/"serve.log").read_text()
                finally:
                    if service.poll() is None:
                        service.kill();service.wait(timeout=10)
            state=invoke("remove","--store",store,"--instance","mixed-demo","--targets",targets,"--delete")
            assert not API.objects
            assert all(u["deleted"] for u in state["units"].values()),state
            invoke("reconcile",*args,"--store",store,"--once")
            assert not API.objects
        finally:
            server.shutdown();server.server_close();thread.join(timeout=5)
    print("PASS: offline receipt, saved plan, real OpenTofu -> HTTP Kubernetes binding, restart, stale plan, revision update, explicit deletion")

if __name__ == "__main__":
    main()
