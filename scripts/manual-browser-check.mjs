// Browser integration against DTOs captured from the real local SQL/Engine test.
// IPC is replayed read-only. This is NOT a Tauri/WebView or live DB browser test.
import assert from "node:assert/strict";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { spawn } from "node:child_process";
import { createServer } from "node:net";
import path from "node:path";
import os from "node:os";

const directory = path.resolve(process.argv[2] ?? "");
const interactive = process.argv.includes("--view");
assert.ok(directory.startsWith(path.resolve(os.tmpdir()) + path.sep), "Evidence must be in the isolated TEMP directory");
const evidence = JSON.parse(await readFile(path.join(directory, "manual-evidence.json"), "utf8"));
assert.equal(evidence.database, "golf_report_migration_test");
assert.match(evidence.schema, /^gr_mig_\d+_\d+$/);
assert.equal(evidence.stages.length, 5);
assert.equal(evidence.commitProof.length, 5);
const browser = process.env.GOLF_TEST_BROWSER ?? "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const port = await new Promise(resolve => {
  const listener = createServer();
  listener.listen(0, "127.0.0.1", () => {
    const selected = listener.address().port;
    listener.close(() => resolve(selected));
  });
});
const vite = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", String(port)], { windowsHide: true, stdio: "ignore" });
const profile = path.join(directory, `browser-profile-${Date.now()}`);
await mkdir(profile, { recursive: true });
const chrome = spawn(browser, [...(interactive ? [] : ["--headless=new"]), "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--remote-debugging-address=127.0.0.1", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--window-size=1600,1200", "about:blank"], { windowsHide: !interactive, stdio: "ignore" });
let socket;
const results = [];
const exceptions = [];
try {
  let debugPort;
  for (let retry = 0; retry < 150; retry++) {
    try { debugPort = (await readFile(path.join(profile, "DevToolsActivePort"), "utf8")).split("\n")[0]; break; } catch { await sleep(200); }
  }
  assert.ok(debugPort, "Browser CDP did not start");
  const pages = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json();
  socket = new WebSocket(pages.find(page => page.type === "page").webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.addEventListener("open", resolve, { once: true }); socket.addEventListener("error", reject, { once: true }); });
  let next = 0;
  const requests = new Map();
  socket.addEventListener("message", event => {
    const message = JSON.parse(String(event.data));
    if (message.method === "Runtime.exceptionThrown") exceptions.push(message.params.exceptionDetails);
    if (!message.id) return;
    const pending = requests.get(message.id);
    if (!pending) return;
    requests.delete(message.id); clearTimeout(pending.timer);
    message.error ? pending.reject(new Error(JSON.stringify(message.error))) : pending.resolve(message.result);
  });
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++next;
    const timer = setTimeout(() => { requests.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, 30000);
    requests.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => {
    const response = await call("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true });
    if (response.exceptionDetails) throw new Error(JSON.stringify(response.exceptionDetails));
    return response.result.value;
  };
  const wait = async expression => {
    for (let retry = 0; retry < 150; retry++) { if (await evaluate(expression)) return; await sleep(200); }
    throw new Error(`DOM condition failed: ${expression}\n${await evaluate("document.body.innerText")}`);
  };
  await call("Page.enable"); await call("Runtime.enable");
  await call("Page.addScriptToEvaluateOnNewDocument", { source: `
    window.__manualEvidence = ${JSON.stringify(evidence)};
    window.__manualCalls = [];
    document.addEventListener('DOMContentLoaded', () => {
      const panel = document.createElement('div');
      panel.style.cssText = 'position:fixed;bottom:8px;right:8px;z-index:100000;background:#102a43;color:white;padding:8px;font:12px sans-serif';
      panel.textContent = 'LOCAL SQL snapshot / IPC replay — NOT Tauri / PLC '; 
      for (const [label,index] of [['A',0],['B',1],['C (5)',4]]) {
        const button = document.createElement('button'); button.textContent=label;
        button.onclick=()=>{sessionStorage.setItem('manual-stage',String(index)); location.assign('/monitor');};
        panel.appendChild(button);
      }
      document.body.appendChild(panel);
    });
    window.__TAURI_INTERNALS__ = { invoke: async (command, args = {}) => {
      window.__manualCalls.push({ command, args });
      const data = window.__manualEvidence;
      const stage = data.stages[Number(sessionStorage.getItem('manual-stage') || 0)];
      const request = args.request || {};
      switch (command) {
        case 'get_database_status': return {state:'CONNECTED',message:'LOCAL SQL snapshot / read-only IPC replay'};
        case 'get_as200_worker_status': return stage.status;
        case 'get_plc_connection_settings': return {
          configured:true,operatorSku:'MANUAL-QC',endpoint:{ip:'127.0.0.1',port:Number(data.endpoint.split(':')[1]),unitId:1},
          parameters:{plcType:'Delta AS200',protocol:'Modbus TCP',layoutProfile:'as200',controlStart:25000,bufferCount:2,recordSize:240,wordOrder:'LOW',modbusAddressOffset:0,pollIntervalMs:500,timeoutMs:2000,heartbeatTimeoutMs:5000,retryIntervalMs:1000,readOnly:false,ackEnabled:true}
        };
        case 'get_measurement_by_id': {
          const detail = stage.measurements.find(item => item.id === args.id);
          if (!detail) throw new Error('Unknown SQL Measurement ID');
          return detail;
        }
        case 'get_measurement_history_page':
        case 'get_measurements': {
          if (request.productCode && request.productCode !== 'MANUAL-QC') throw new Error('Unsupported snapshot SKU');
          const size = request.pageSize || 20, page = request.page || 1;
          return {...stage.history, items:stage.history.items.slice((page-1)*size,page*size),page,pageSize:size};
        }
        case 'get_measurement_skus': return {items:['MANUAL-QC'],hasMore:false};
        case 'get_measurement_sku_summary': return stage.skuSummary;
        case 'get_measurement_statistics': return stage.statistics;
        case 'get_measurement_quality_statistics': return stage.quality;
        case 'get_measurement_report': return stage.measurements;
        default: throw new Error('Write/unknown IPC prohibited in browser test: ' + command);
      }
    }};
  ` });
  for (let retry = 0; retry < 100; retry++) { try { if ((await fetch(`http://127.0.0.1:${port}`)).ok) break; } catch {} await sleep(200); }
  const navigate = async pathname => call("Page.navigate", { url: `http://127.0.0.1:${port}${pathname}` });
  const screenshot = async name => {
    const shot = await call("Page.captureScreenshot", { format: "png", captureBeyondViewport: true });
    await writeFile(path.join(directory, `${name}.png`), Buffer.from(shot.data, "base64"));
  };
  const rows = selector => evaluate(`Array.from(document.querySelectorAll(${JSON.stringify(selector)} + ' tbody tr[data-row-key]')).map(row => ({id:row.dataset.rowKey,text:row.innerText}))`);
  for (const [stageIndex, label] of [[0,"A"],[1,"B"],[4,"C"]]) {
    await navigate("/monitor");
    await wait("!!document.querySelector('.monitor-recent tbody tr[data-row-key]')");
    // Change the captured backend state without reloading the mounted page:
    // MainLayout's real polling and lastSavedId effects must refresh the rows.
    await evaluate(`sessionStorage.setItem('manual-stage',${stageIndex})`);
    const stage = evidence.stages[stageIndex];
    await wait(`document.querySelectorAll('.monitor-recent tbody tr[data-row-key]').length === ${stage.count}`);
    const monitor = await rows(".monitor-recent");
    assert.equal(monitor[0].id, String(stage.history.items[0].id));
    assert.ok(monitor[0].text.includes(stageIndex === 0 ? "#0" : stageIndex === 1 ? "#12" : "#55"));
    assert.ok(!(await evaluate("document.body.innerText")).includes("Raw PLC đã nhận"));
    if (stageIndex === 1) {
      await wait("document.querySelectorAll('.monitor-point-table tbody tr[data-row-key]').length === 1");
      const cells = await evaluate("Array.from(document.querySelector('.monitor-point-table tbody tr[data-row-key]').querySelectorAll('td')).map(td=>td.innerText)");
      assert.deepEqual(cells, ["TIP · P1","10","0","—","—","1","CHƯA ĐO"]);
      results.push({test:"B monitor decision vs actual attempt",cells});
    }
    await screenshot(`manual-${label}-monitor`);
    await evaluate("Array.from(document.querySelectorAll('.ant-menu-item')).find(item=>item.innerText.includes('Lịch sử')).click()");
    await wait(`location.pathname === '/measurements' && document.querySelectorAll('.history-detail-table tbody tr[data-row-key]').length === ${stage.count}`);
    const history = await rows(".history-detail-table");
    assert.deepEqual(history.map(item=>item.id), stage.history.items.map(item=>String(item.id)));
    await screenshot(`manual-${label}-history`);
    const id = stage.history.items[0].id;
    await evaluate(`document.querySelector('.history-detail-table tr[data-row-key="${id}"] button').click()`);
    await wait(`location.pathname === '/measurements/${id}' && !!document.querySelector('.measurement-details')`);
    if (stageIndex === 1) {
      await wait("!!document.querySelector('.measurement-point-table .ant-table-row-expand-icon')");
      await evaluate("document.querySelector('.measurement-point-table .ant-table-row-expand-icon').click()");
      await wait("!!document.querySelector('.measurement-attempt-table tbody tr[data-row-key]')");
      const attempt = await evaluate("Array.from(document.querySelector('.measurement-attempt-table tbody tr[data-row-key]').querySelectorAll('td')).map(td=>td.innerText)");
      assert.deepEqual(attempt,["1","12","0"]);
      results.push({test:"B detail expanded actual attempt",cells:attempt});
    }
    await screenshot(`manual-${label}-detail`);
    results.push({stage:label,monitor,history,detailPath:await evaluate("location.pathname"),statistics:stage.statistics});
  }
  assert.equal(exceptions.length, 0, JSON.stringify(exceptions));
  const report = {mode:"Chrome headless / real React routes / SQL DTO snapshot / read-only IPC replay; NOT Tauri E2E",schema:evidence.schema,results,exceptions};
  await writeFile(path.join(directory,"browser-evidence.json"),JSON.stringify(report,null,2));
  console.log(JSON.stringify({result:"PASS",mode:report.mode,stages:3, screenshots:9, directory}));
  if (interactive) {
    console.log("Read-only browser replay is open. A/B/C selects SQL snapshots; close this test browser to stop.");
    await new Promise(resolve => chrome.once("exit", resolve));
  } else { await call("Browser.close"); }
} finally {
  socket?.close();
  chrome.kill(); vite.kill();
}
