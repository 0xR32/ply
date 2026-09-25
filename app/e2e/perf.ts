// bun app/e2e/perf.ts <p1|p2|idle|p5|soak> [n]: the WP11 runs on the full app and a release plyd (docs/perf.md).
import { readdirSync, readFileSync, symlinkSync } from 'node:fs';
import { join } from 'node:path';
import type { Pane } from '../src/ipc/proto.gen';
import { connectPane } from '../src/terminal/data-client';
import { AppProcess, alive, Observer, REPO, Sandbox, until } from './harness';

interface Run {
  sb: Sandbox;
  app: AppProcess;
  obs: Observer;
}

type Fields = Record<string, number>;

const FIXTURES = join(REPO, 'crates', 'term', 'tests', 'fixtures');
const DEBUG_PLYD = join(REPO, 'target', 'debug', 'plyd');

function pct(values: number[], q: number): number {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] ?? Number.NaN;
}

function summary(values: number[], unit: string, digits = 2): string {
  const f = (v: number) => v.toFixed(digits);
  return `n=${values.length} p50 ${f(pct(values, 0.5))} p90 ${f(pct(values, 0.9))} p99 ${f(pct(values, 0.99))} max ${f(Math.max(...values))} ${unit}`;
}

async function launchApp(sb: Sandbox, env: Record<string, string> = {}): Promise<AppProcess> {
  const app = await AppProcess.launch(sb.env(env), join(sb.root, 'app.stderr'));
  void app.child.exited.then((code) =>
    console.log(
      `  (${new Date().toISOString()} the app exited: code ${code}, ${app.child.signalCode ?? 'no signal'})`,
    ),
  );
  await until(
    async () => (await app.has('pane-grid')) || (await app.has('pane-grid-empty')),
    'the app to load',
    20_000,
  );
  return app;
}

async function start(env: Record<string, string> = {}): Promise<Run> {
  const sb = new Sandbox();
  const app = await launchApp(sb, env);
  const obs = await Observer.connect(sb.controlSocket);
  return { sb, app, obs };
}

async function finish(run: Run): Promise<void> {
  run.obs.close();
  await run.app.kill();
  await run.sb.remove();
}

/** `n` panes of `cli` in one new tab (C1, as the app's ⌘N does); the app shows them through `pane.added`. */
async function oneTab(run: Run, n: number, cli: Pane['cli'] = 'shell'): Promise<Pane[]> {
  const out: Pane[] = [];
  for (let i = 0; i < n; i++) {
    const first = out[0];
    out.push(
      await run.obs.client.request('pane.create', {
        workspace_id: run.obs.workspaceId,
        cli,
        cwd: run.sb.home,
        ...(first ? { tab_id: first.tab_id } : {}),
      }),
    );
  }
  return out;
}

async function prompt(run: Run, id: number): Promise<void> {
  await until(
    async () => /[$#] ?$/.test((await run.app.textIn(`terminal-${id}-grid`)).trimEnd()),
    `a prompt in pane ${id}`,
  );
}

/** Focuses pane `id` with a click on its header and types `line` and Enter through the app. */
async function typeLine(run: Run, id: number, line: string): Promise<void> {
  await run.app.click(`pane-${id}-focus`);
  await run.obs.waitFocus(id, `pane ${id} focused`);
  await run.app.type(`${line}\n`);
}

/** Links the debug plyd and the recorded streams into the sandbox home under lower-case names that type safely. */
function linkReplay(sb: Sandbox): string[] {
  symlinkSync(DEBUG_PLYD, join(sb.home, 'feed'));
  const streams = readdirSync(FIXTURES)
    .filter((f) => f.endsWith('.bytes'))
    .sort();
  return streams.map((f, i) => {
    const name = `s${i}.bytes`;
    symlinkSync(join(FIXTURES, f), join(sb.home, name));
    return name;
  });
}

/** Starts `plyd --replay`'s feeder (debug builds) in each pane, round robin over the recorded streams, at `speed` × 4 KiB/s. */
async function replay(run: Run, panes: Pane[], speed: number): Promise<void> {
  const streams = linkReplay(run.sb);
  for (const [i, p] of panes.entries()) {
    await prompt(run, p.id);
    await typeLine(
      run,
      p.id,
      `exec ./feed --replay-feed ~/${streams[i % streams.length]} --speed ${speed}`,
    );
  }
}

function appLog(sb: Sandbox, message: string, since: number): Fields[] {
  const dir = join(sb.plyHome, 'logs');
  const out: Fields[] = [];
  for (const f of readdirSync(dir).filter((n) => n.startsWith('app.'))) {
    for (const line of readFileSync(join(dir, f), 'utf8').split('\n')) {
      const m = line.match(/^(\S+) INFO (.+?) (\{.*\})$/);
      if (!m || m[2] !== message || Date.parse(m[1] as string) < since) continue;
      out.push(JSON.parse(m[3] as string) as Fields);
    }
  }
  return out;
}

/** `P2: key frame to pty write` lines of plyd's log after `since`: [wall-clock µs of the write, latency µs]. */
function p2Lines(sb: Sandbox, since: number): [number, number][] {
  const dir = join(sb.plyHome, 'logs');
  const out: [number, number][] = [];
  for (const f of readdirSync(dir).filter((n) => n.startsWith('plyd.'))) {
    for (const line of readFileSync(join(dir, f), 'utf8').split('\n')) {
      if (!line.includes('P2: key frame to pty write')) continue;
      const at = line.match(/^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)\.(\d+)Z/);
      const lat = line.match(/latency_us=(\d+)/);
      if (!at || !lat) continue;
      const us =
        Date.parse(`${at[1]}Z`) * 1000 + Number((at[2] as string).padEnd(6, '0').slice(0, 6));
      if (us / 1000 >= since) out.push([us, Number(lat[1])]);
    }
  }
  return out.sort((a, b) => a[0] - b[0]);
}

/** Cumulative CPU seconds of `pid` (`ps -o time`). */
function cpuSeconds(pid: number): number {
  const out = Bun.spawnSync(['ps', '-o', 'time=', '-p', String(pid)])
    .stdout.toString()
    .trim();
  return out
    .split(':')
    .map(Number)
    .reduce((acc, v) => acc * 60 + v, 0);
}

/** Physical footprint of `pid` in MB (`footprint -p`, the number Activity Monitor calls Memory). */
function footprintMB(pid: number): number {
  const out = Bun.spawnSync(['footprint', '-p', String(pid)]).stdout.toString();
  const m = out.match(/Footprint:\s+([\d.]+)\s+(KB|MB|GB)/);
  if (!m) return Number.NaN;
  const scale = m[2] === 'KB' ? 1 / 1024 : m[2] === 'GB' ? 1024 : 1;
  return Number(m[1]) * scale;
}

function plydOf(run: Run): number {
  const pid = run.sb.plydPid();
  if (pid === undefined) throw new Error('plyd is not running');
  return pid;
}

async function p1(seconds: number): Promise<void> {
  const run = await start({ PLY_TERMINAL_STATS: '1' });
  try {
    const panes = await oneTab(run, 6);
    await until(async () => (await run.app.visiblePanes()).length === 6, 'six visible panes');
    await replay(run, panes, 10);
    await Bun.sleep(10_000);
    const since = Date.now();
    await Bun.sleep(seconds * 1000);
    const w = appLog(run.sb, 'frame stats', since);
    const p99s = w.map((x) => x.draw_p99_ms ?? Number.NaN).filter(Number.isFinite);
    const maxes = w.map((x) => x.draw_max_ms ?? Number.NaN).filter(Number.isFinite);
    const sum = (k: string) => w.reduce((a, x) => a + (x[k] ?? 0), 0);
    console.log(
      `P1 six replayed panes at 10× for ${seconds} s (one line per second of GPUI draw stats)`,
    );
    console.log(`  draw p99 per second: ${summary(p99s, 'ms')}`);
    console.log(`  draw max per second: ${summary(maxes, 'ms')}`);
    console.log(`  frames drawn: ${(sum('frames') / w.length).toFixed(1)} per second`);
    console.log(
      `  main-thread stall p99 per second: ${summary(
        w.map((x) => x.stall_p99_ms ?? 0),
        'ms',
      )}`,
    );
    console.log(
      `  main-thread stall max: ${Math.max(...w.map((x) => x.stall_max_ms ?? 0)).toFixed(1)} ms`,
    );
    console.log(
      `  stalls over 16.7 ms: ${sum('stalls_over_16ms')}, over 33.3 ms: ${sum('stalls_over_33ms')} in ${w.length} s`,
    );
    console.log(
      `  app CPU ${cpuSeconds(run.app.pid).toFixed(1)} s, plyd CPU ${cpuSeconds(plydOf(run)).toFixed(1)} s cumulative`,
    );
  } finally {
    await finish(run);
  }
}

async function typeKeys(run: Run, n: number): Promise<number[]> {
  const sent: number[] = [];
  for (let i = 0; i < n; i++) {
    sent.push((performance.timeOrigin + performance.now()) * 1000);
    await run.app.automation.call('keystrokes', { keys: 'a' });
    await Bun.sleep(10);
  }
  await Bun.sleep(500);
  return sent;
}

function p2Report(label: string, run: Run, since: number, sent: number[]): void {
  const lines = p2Lines(run.sb, since).slice(-sent.length);
  const e2e = lines.map(([at], i) => (at - (sent[i] ?? at)) / 1000);
  console.log(
    `P2 ${label}: plyd KEY frame → write(2) ${summary(
      lines.map(([, l]) => l / 1000),
      'ms',
      3,
    )}`,
  );
  console.log(`   ${label}: automation keystroke sent → write(2) ${summary(e2e, 'ms', 3)}`);
}

async function p2(): Promise<void> {
  const run = await start({ PLY_LOG: 'debug' });
  try {
    const panes = await oneTab(run, 6);
    const shell = panes[0] as Pane;
    for (const p of panes) await prompt(run, p.id);
    await run.app.click(`pane-${shell.id}-focus`);
    await run.obs.waitFocus(shell.id, 'the shell focused');
    const trip: number[] = [];
    for (let i = 0; i < 100; i++) {
      const t0 = performance.now();
      await run.app.automation.call('getBounds', { elementId: 1 });
      trip.push(performance.now() - t0);
      await Bun.sleep(10);
    }
    console.log(`P2 one automation round trip (getBounds): ${summary(trip, 'ms', 3)}`);
    let since = Date.now();
    p2Report('6 idle panes', run, since, await typeKeys(run, 300));
    await run.app.type('\u0003');
    await replay(run, panes.slice(1), 10);
    await run.app.click(`pane-${shell.id}-focus`);
    await run.obs.waitFocus(shell.id, 'the shell focused again');
    await Bun.sleep(5_000);
    since = Date.now();
    p2Report('5 panes replaying at 10×', run, since, await typeKeys(run, 300));
  } finally {
    await finish(run);
  }
}

/** Attaches at 168 × 50, runs `command` and detaches once the scrollback holds `rows` rows; returns the count seen. */
async function fillAt168(socketPath: string, paneId: number, command: string, rows: number) {
  return new Promise<number>((resolve, reject) => {
    let seen = 0;
    let sent = false;
    const timer = setTimeout(() => {
      conn.close();
      reject(new Error(`pane ${paneId} reached ${seen} scrollback rows, not ${rows}`));
    }, 60_000);
    const conn = connectPane(
      { socketPath, paneId, size: { cols: 168, rows: 50, cellWidthPx: 8, cellHeightPx: 16 } },
      {
        onFrames: (frames) => {
          for (const f of frames) {
            if (f.kind === 'snapshot' || f.kind === 'delta') seen = f.scrollbackRows;
          }
          if (seen < rows) return;
          clearTimeout(timer);
          conn.close();
          resolve(seen);
        },
        onState: (s) => {
          if (s.kind === 'refused') reject(new Error(`attach refused for pane ${paneId}`));
          if (s.kind !== 'attached' || sent) return;
          sent = true;
          conn.send({ kind: 'inputRaw', bytes: new TextEncoder().encode(`${command}\r`) });
        },
      },
    );
  });
}

async function idle(minutes: number): Promise<void> {
  const sb = new Sandbox();
  let app = await launchApp(sb);
  const obs = await Observer.connect(sb.controlSocket);
  let run: Run = { sb, app, obs };
  try {
    await Bun.sleep(5_000);
    const plyd = plydOf(run);
    const plyd0 = footprintMB(plyd);
    const app0 = footprintMB(app.pid);
    console.log(
      `P4 baseline: plyd ${plyd0.toFixed(1)} MB with no pane, app ${app0.toFixed(1)} MB with no pane`,
    );
    const [appFloor, plydFloor] = [cpuSeconds(app.pid), cpuSeconds(plyd)];
    await Bun.sleep(60_000);
    const share = (pid: number, from: number) => (((cpuSeconds(pid) - from) / 60) * 100).toFixed(3);
    console.log(
      `P3 floor, no pane, 60 s: app ${share(app.pid, appFloor)} % of a core, plyd ${share(plyd, plydFloor)} %`,
    );
    await app.quit();
    const panes = await oneTab(run, 6);
    const data = join(sb.plyHome, 'run', 'data.sock');
    await Bun.sleep(1_500);
    const rows: number[] = [];
    for (const p of panes) rows.push(await fillAt168(data, p.id, 'seq -f %0168.0f 1 10000', 9_950));
    console.log(`P4 fill: scrollback rows per pane at 168 × 50: ${rows.join(', ')}`);
    await Bun.sleep(15_000);
    const filled = footprintMB(plyd);
    console.log(
      `P4 filled: six panes of 10 000 lines × 168 columns at 168 × 50, no view: plyd ${filled.toFixed(1)} MB (${((filled - plyd0) / 6).toFixed(2)} MB per pane)`,
    );
    app = await launchApp(sb);
    run = { sb, app, obs };
    await until(async () => (await app.visiblePanes()).length === 6, 'six visible panes');
    for (const p of panes) {
      await until(
        async () => (await app.textIn(`terminal-${p.id}`)).includes('10000'),
        `pane ${p.id}'s last line`,
        20_000,
      );
    }
    const t0 = Date.now();
    let cpuFrom: [number, number] | null = null;
    for (let m = 1; m <= minutes; m++) {
      await Bun.sleep(60_000);
      if (m === minutes - 5) cpuFrom = [cpuSeconds(app.pid), cpuSeconds(plyd)];
      console.log(
        `  minute ${m}: app ${footprintMB(app.pid).toFixed(1)} MB, plyd ${footprintMB(plyd).toFixed(1)} MB, app CPU ${cpuSeconds(app.pid).toFixed(2)} s, plyd CPU ${cpuSeconds(plyd).toFixed(2)} s`,
      );
    }
    const window = cpuFrom ? 300 : (Date.now() - t0) / 1000;
    const [appCpu0, plydCpu0] = cpuFrom ?? [0, 0];
    const appCpu = ((cpuSeconds(app.pid) - appCpu0) / window) * 100;
    const plydCpu = ((cpuSeconds(plyd) - plydCpu0) / window) * 100;
    console.log(
      `P3 six idle panes, last ${window.toFixed(0)} s: app ${appCpu.toFixed(3)} % of a core, plyd ${plydCpu.toFixed(3)} % of a core`,
    );
    const plydEnd = footprintMB(plyd);
    console.log(
      `P4 after ${minutes} min idle: app ${footprintMB(app.pid).toFixed(1)} MB with 6 visible panes; plyd ${plydEnd.toFixed(1)} MB = ${((plydEnd - plyd0) / 6).toFixed(2)} MB per pane over its ${plyd0.toFixed(1)} MB start`,
    );
  } finally {
    await finish(run);
  }
}

async function p5(switches: number): Promise<void> {
  const run = await start();
  try {
    const tabs: Pane[][] = [];
    for (let t = 0; t < 4; t++) tabs.push(await oneTab(run, 4));
    for (const [t, panes] of tabs.entries()) {
      await run.app.keys(`cmd-${t + 1}`);
      for (const [i, p] of panes.entries()) {
        await prompt(run, p.id);
        const streams = i === 0 && (t === 1 || t === 3);
        await typeLine(
          run,
          p.id,
          streams
            ? `while :; do echo p5m${p.id}x; sleep 0.01; done`
            : `clear; seq 1 60; echo p5m${p.id}x`,
        );
      }
    }
    const shows = async (panes: Pane[]) => {
      const painted = (await run.app.painted()).join('\n');
      return panes.every((p) => painted.includes(`p5m${p.id}x`));
    };
    for (let t = 0; t < 4; t++) {
      await run.app.keys(`cmd-${t + 1}`);
      await until(() => shows(tabs[t] as Pane[]), `tab ${t + 1} painted`);
    }
    const trip: number[] = [];
    for (let i = 0; i < 20; i++) {
      const t0 = performance.now();
      await run.app.painted();
      trip.push(performance.now() - t0);
    }
    const times: number[] = [];
    for (let i = 0; i < switches; i++) {
      const t = i % 4;
      const t0 = performance.now();
      await run.app.automation.call('keystrokes', { keys: `cmd-${t + 1}` });
      // `until` sleeps 20 ms between checks, too coarse for a 50 ms target; one getPaintedText is under 1 ms.
      while (!(await shows(tabs[t] as Pane[]))) {
        if (performance.now() - t0 > 5_000) throw new Error(`tab ${t + 1} never painted`);
      }
      times.push(performance.now() - t0);
      await Bun.sleep(150);
    }
    console.log('P5 4 tabs × 4 panes, 2 streaming: ⌘1–⌘4 → every pane of the new tab painted');
    console.log(`  switch → first full frame: ${summary(times, 'ms', 1)}`);
    console.log(`  (one automation round trip, getPaintedText: ${summary(trip, 'ms', 1)})`);
  } finally {
    await finish(run);
  }
}

const claudeSession = (pane: number) => `00000000-0000-4000-8000-${String(pane).padStart(12, '0')}`;

async function soak(minutes: number): Promise<void> {
  const run = await start();
  try {
    const [c1, c2] = (await oneTab(run, 2, 'claude')) as [Pane, Pane];
    const add = (cli: Pane['cli']) =>
      run.obs.client.request('pane.create', {
        workspace_id: run.obs.workspaceId,
        cli,
        cwd: run.sb.home,
        tab_id: c1.tab_id,
      });
    const cx = await add('codex');
    const shells = [await add('shell'), await add('shell'), await add('shell')];
    await until(async () => (await run.app.visiblePanes()).length === 6, 'six visible panes');
    const loads = [
      'while :; do date; sleep 1; done',
      'while :; do seq 1 3000; sleep 5; done',
      'while :; do ls -la /usr/bin; sleep 2; done',
    ];
    for (const [i, s] of shells.entries()) {
      await prompt(run, s.id);
      await typeLine(run, s.id, loads[i] as string);
    }
    await run.sb.fake(cx.id).send('session');
    const plyd = plydOf(run);
    const end = Date.now() + minutes * 60_000;
    const cycles = new Map<number, number>();
    const stuck: string[] = [];
    const wait = async (id: number, status: Pane['status'], what: string) => {
      try {
        await run.obs.waitPane(id, (p) => p.status === status, what, 15_000);
      } catch (e) {
        stuck.push(`${new Date().toISOString()} ${String(e)}`);
      }
    };
    const answer = async (id: number) => {
      try {
        await until(() => run.app.has(`answer-${id}-1`), `the answer button of pane ${id}`);
        await run.app.click(`answer-${id}-1`);
      } catch (e) {
        stuck.push(`${new Date().toISOString()} ${String(e)}`);
      }
    };
    const claudeCycle = async (p: Pane) => {
      const fake = run.sb.fake(p.id);
      const base = {
        session_id: claudeSession(p.id),
        transcript_path: '/Users/example/.claude/projects/example/session.jsonl',
        cwd: (await run.obs.panes()).find((x) => x.id === p.id)?.cwd ?? p.cwd,
        permission_mode: 'default',
      };
      const call = {
        tool_name: 'Write',
        tool_input: { file_path: '/Users/example/project/a.txt', content: 'hi\n' },
        tool_use_id: `toolu_soak${p.id}`,
      };
      await fake.hook('UserPromptSubmit', {
        ...base,
        hook_event_name: 'UserPromptSubmit',
        prompt: 'go',
      });
      await wait(p.id, 'running', `claude ${p.id} running`);
      await fake.send('spin 10');
      await fake.hook('PermissionRequest', {
        ...base,
        hook_event_name: 'PermissionRequest',
        tool_name: call.tool_name,
        tool_input: call.tool_input,
      });
      await wait(p.id, 'waiting_permission', `claude ${p.id} waiting`);
      await answer(p.id);
      await wait(p.id, 'running', `claude ${p.id} answered`);
      await fake.hook('PostToolUse', { ...base, hook_event_name: 'PostToolUse', ...call });
      for (let i = 0; i < 20; i++) await fake.send(`out claude ${p.id} line ${i}`);
      await fake.hook('Stop', { ...base, hook_event_name: 'Stop' });
      await wait(p.id, 'idle', `claude ${p.id} idle`);
      cycles.set(p.id, (cycles.get(p.id) ?? 0) + 1);
    };
    let turn = 0;
    const codexCycle = async () => {
      const fake = run.sb.fake(cx.id);
      const id = `00000000-0000-7000-8000-0000000${String(++turn).padStart(5, '0')}`;
      await fake.send(`turn task_started ${id}`);
      await wait(cx.id, 'running', 'codex running');
      for (let i = 0; i < 20; i++) await fake.send(`out codex line ${i}`);
      await fake.send('osc9 Approval requested: echo hi');
      await wait(cx.id, 'waiting_permission', 'codex waiting');
      await answer(cx.id);
      await wait(cx.id, 'running', 'codex answered');
      await fake.send(`turn task_complete ${id}`);
      await wait(cx.id, 'idle', 'codex idle');
      cycles.set(cx.id, (cycles.get(cx.id) ?? 0) + 1);
    };
    const samples: string[] = [];
    let nextSample = Date.now();
    while (Date.now() < end) {
      if (!alive(run.app.pid) || !alive(plyd))
        throw new Error('the app or plyd died during the soak');
      await Promise.all([claudeCycle(c1), claudeCycle(c2), codexCycle()]);
      if (Date.now() >= nextSample) {
        nextSample += 60_000;
        const line = `  ${new Date().toISOString().slice(11, 19)} app ${footprintMB(run.app.pid).toFixed(1)} MB CPU ${cpuSeconds(run.app.pid).toFixed(1)} s · plyd ${footprintMB(plyd).toFixed(1)} MB CPU ${cpuSeconds(plyd).toFixed(1)} s · cycles ${[...cycles.values()].join('/')}`;
        samples.push(line);
        console.log(line);
      }
      await Bun.sleep(2_000);
    }
    const statuses = (await run.obs.panes()).map((p) => `${p.id}:${p.cli}:${p.status}`);
    console.log(
      `soak ${minutes} min: cycles per agent ${[...cycles.entries()].map(([k, v]) => `${k}=${v}`).join(' ')}`,
    );
    console.log(
      `  final statuses ${statuses.join(' ')}; app alive ${alive(run.app.pid)}, plyd alive ${alive(plyd)}`,
    );
    console.log(
      `  stuck waits: ${stuck.length}${stuck.length ? `\n    ${stuck.join('\n    ')}` : ''}`,
    );
  } finally {
    await finish(run);
  }
}

const [mode, arg] = process.argv.slice(2);
const n = Number(arg);
switch (mode) {
  case 'p1':
    await p1(Number.isFinite(n) && n > 0 ? n : 60);
    break;
  case 'p2':
    await p2();
    break;
  case 'idle':
    await idle(Number.isFinite(n) && n > 0 ? n : 10);
    break;
  case 'p5':
    await p5(Number.isFinite(n) && n > 0 ? n : 40);
    break;
  case 'soak':
    await soak(Number.isFinite(n) && n > 0 ? n : 20);
    break;
  default:
    console.log('usage: bun app/e2e/perf.ts <p1|p2|idle|p5|soak> [seconds|minutes|switches]');
}
