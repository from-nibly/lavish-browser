#!/usr/bin/env python3
"""Stage the final installed Excalidraw residual after automated evidence passes."""
import argparse, json, os, pathlib, shutil, subprocess, tempfile, time

p=argparse.ArgumentParser(); p.add_argument('--prefix',type=pathlib.Path,required=True); p.add_argument('--artifact',type=pathlib.Path,required=True); p.add_argument('--evidence',type=pathlib.Path,required=True); p.add_argument('--display',required=True); a=p.parse_args()
a.evidence.mkdir(parents=True,exist_ok=True); isolated=pathlib.Path(tempfile.mkdtemp(prefix='lavish-manual-')); runtime=isolated/'r'; state=isolated/'s'; config=isolated/'c'
for d in (runtime,state,config): d.mkdir(mode=0o700)
env=os.environ.copy(); env.update({'DISPLAY':a.display,'XDG_RUNTIME_DIR':str(runtime),'XDG_STATE_HOME':str(state),'XDG_CONFIG_HOME':str(config),'LAVISH_BROWSER_EXECUTABLE':str((a.prefix/'bin/lavish-browser').resolve())})
for k in ('ZELLIJ','ZELLIJ_SESSION_NAME','ZELLIJ_PANE_ID'): env.pop(k,None)
browser_log=(a.evidence/'browser.log').open('w'); browser=subprocess.Popen([str(a.prefix/'bin/lavish-browser')],env=env,stdout=browser_log,stderr=subprocess.STDOUT)
socket=runtime/'lavish-browser/control.sock'; deadline=time.monotonic()+20
while not socket.exists() and time.monotonic()<deadline: time.sleep(.1)
if not socket.exists(): raise SystemExit('installed browser socket did not start')
opened=subprocess.run([str(a.prefix/'bin/lavish-open'),str(a.artifact.resolve())],env=env,text=True,capture_output=True,timeout=180)
(a.evidence/'launcher.log').write_text(opened.stdout+opened.stderr)
if opened.returncode: raise SystemExit('installed launcher failed')
time.sleep(6)
poll=subprocess.Popen(['npx','-y','lavish-axi','poll',str(a.artifact.resolve())],env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
result=a.evidence/'manual-result.json'; ready={'status':'ready','evidence':str(a.evidence.resolve()),'artifact':str(a.artifact.resolve()),'display':a.display,'browser_pid':browser.pid,'poll_pid':poll.pid,'result_path':str(result.resolve()),'feedback_note':'manual Excalidraw persistence confirmation'}
(a.evidence/'manual-ready.json').write_text(json.dumps(ready,indent=2)); print('MANUAL_EXCALIDRAW_READY '+json.dumps(ready),flush=True)
deadline=time.monotonic()+1800
while not result.exists() and time.monotonic()<deadline: time.sleep(.5)
if not result.exists(): raise SystemExit('manual result timed out')
observation=json.loads(result.read_text()); required=observation.get('status')=='pass' and all(observation.get(k) is True for k in ('edit_visible','persisted_after_reload','feedback_queued'))
if not required: raise SystemExit(f'manual observation failed: {observation}')
out,err=poll.communicate(timeout=120); payload={'exit_code':poll.returncode,'stdout':out,'stderr':err}; (a.evidence/'poll-whiteboard.json').write_text(json.dumps(payload,indent=2))
if poll.returncode or 'manual Excalidraw persistence confirmation' not in out: raise SystemExit('matching real whiteboard poll payload missing')
(a.evidence/'PASS').write_text('installed Excalidraw persistence and real poll passed\n')
browser.terminate(); browser.wait(timeout=15); browser_log.close(); shutil.copytree(state,a.evidence/'xdg-state',dirs_exist_ok=True); shutil.rmtree(isolated,ignore_errors=True)
