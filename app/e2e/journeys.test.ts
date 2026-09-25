import { afterEach, describe, expect, test } from 'bun:test';
import { existsSync, mkdirSync, readFileSync, realpathSync } from 'node:fs';
import { join } from 'node:path';
import type { Cli, Pane } from '../src/ipc/proto.gen';
import { AppProcess, alive, Observer, Sandbox, until } from './harness';

// Spec 14's journeys on the full app against a cargo-built plyd; `just e2e` runs them (a GUI session is needed).
const E2E = process.env.PLY_E2E === '1';
const JOURNEY_MS = 90_000;
const cleanups: (() => Promise<void> | void)[] = [];

afterEach(async () => {
  for (const c of cleanups.splice(0).reverse()) await c();
});

interface Journey {
  sb: Sandbox;
  app: AppProcess;
  obs: Observer;
}

async function launch(sb: Sandbox): Promise<AppProcess> {
  const app = await AppProcess.launch(sb.env(), join(sb.root, 'app.stderr'));
  cleanups.push(() => app.kill());
  await until(
    async () => (await app.has('pane-grid')) || (await app.has('pane-grid-empty')),
    'the app to load its session',
    20_000,
  );
  return app;
}

async function start(): Promise<Journey> {
  const sb = new Sandbox();
  cleanups.push(() => sb.remove());
  const app = await launch(sb);
  const obs = await Observer.connect(sb.controlSocket);
  cleanups.push(() => obs.close());
  return { sb, app, obs };
}

/** Opens the ⌘T/⌘N form, picks `cli`, runs `fill`, submits with ⌘⏎ and returns plyd's record of the new pane. */
async function open(
  j: Journey,
  chord: 'cmd-t' | 'cmd-n',
  cli: Exclude<Cli, 'shell'> = 'claude',
  fill?: () => Promise<void>,
): Promise<Pane> {
  const before = await j.obs.panes();
  await j.app.keys(chord);
  await until(() => j.app.has('new-pane'), 'the new-pane form');
  if (cli === 'codex') await j.app.keys('right');
  await fill?.();
  await j.app.keys('cmd-enter');
  const pane = await until(async () => {
    const now = await j.obs.panes();
    return now.length === before.length + 1 ? now.at(-1) : undefined;
  }, `the new ${cli} pane`);
  await until(async () => !(await j.app.has('new-pane')), 'the form to close');
  await until(() => j.app.has(`pane-${pane.id}`), `pane ${pane.id} on screen`);
  return pane;
}

/** ⌘D: a shell in the focused pane's directory. */
async function terminalHere(j: Journey): Promise<Pane> {
  const before = (await j.obs.panes()).length;
  await j.app.keys('cmd-d');
  const panes = await j.obs.waitCount(before + 1, 'the ⌘D shell');
  const pane = panes.at(-1) as Pane;
  await until(() => j.app.has(`pane-${pane.id}`), `shell pane ${pane.id} on screen`);
  return pane;
}

const statusOf = (j: Journey, id: number) => j.app.textIn(`pane-${id}-status`);

async function waitStatusLabel(j: Journey, id: number, label: string): Promise<void> {
  await until(async () => (await statusOf(j, id)).startsWith(label), `pane ${id} to show ${label}`);
}

async function waitScreen(j: Journey, id: number, needle: string, timeoutMs = 10_000) {
  await until(
    async () => (await j.app.textIn(`terminal-${id}`)).includes(needle),
    `"${needle}" in pane ${id}'s terminal`,
    timeoutMs,
  );
}

/** Milliseconds from `act` until GPUI painted a string containing `needle`: what the user sees, not just the tree. */
async function paintedAfter(j: Journey, act: () => Promise<void>, needle: string): Promise<number> {
  const t0 = performance.now();
  await act();
  await until(
    async () => (await j.app.painted()).some((t) => t.includes(needle)),
    `"${needle}" painted`,
  );
  return performance.now() - t0;
}

const claudeSession = (pane: number) => `00000000-0000-4000-8000-${String(pane).padStart(12, '0')}`;
const codexThread = (pane: number) => `00000000-0000-7000-8000-${String(pane).padStart(12, '0')}`;

function claudePayload(pane: Pane, event: string, extra: Record<string, unknown> = {}) {
  return {
    session_id: claudeSession(pane.id),
    transcript_path: '/Users/example/.claude/projects/example/session.jsonl',
    cwd: pane.cwd,
    permission_mode: 'default',
    hook_event_name: event,
    ...extra,
  };
}

const WRITE_CALL = {
  tool_name: 'Write',
  tool_input: { file_path: '/Users/example/project/a.txt', content: 'hi\n' },
};

/** Makes a fake Claude pane ask for permission: a prompt runs it, then PermissionRequest (6.3 needs `running` first). */
async function claudeAsks(j: Journey, pane: Pane): Promise<void> {
  const fake = j.sb.fake(pane.id);
  await fake.hook('UserPromptSubmit', claudePayload(pane, 'UserPromptSubmit', { prompt: 'go' }));
  await j.obs.waitPane(pane.id, (p) => p.status === 'running', `pane ${pane.id} running`);
  await fake.hook('PermissionRequest', claudePayload(pane, 'PermissionRequest', WRITE_CALL));
}

function repo(sb: Sandbox, name: string): string {
  const dir = join(sb.home, 'code', name);
  mkdirSync(dir, { recursive: true });
  Bun.spawnSync(['git', 'init', '-q', dir]);
  return dir;
}

describe.if(E2E)('journeys on the full app and a real plyd', () => {
  test(
    'J1 first run from the checkout: open a repo, create a claude pane, get a prompt',
    async () => {
      const sb = new Sandbox();
      cleanups.push(() => sb.remove());
      const dir = repo(sb, 'ply');
      expect(sb.plydPid()).toBeUndefined();
      const app = await launch(sb);
      const plyd = sb.plydPid();
      expect(plyd).toBeDefined();
      const command = Bun.spawnSync(['ps', '-o', 'command=', '-p', String(plyd)]).stdout.toString();
      expect(command).toMatch(/\/target\/(release|debug)\/plyd --foreground/);
      expect(await app.textIn('pane-grid-empty')).toContain('No panes yet');
      const obs = await Observer.connect(sb.controlSocket);
      cleanups.push(() => obs.close());
      const j = { sb, app, obs };

      const pane = await open(j, 'cmd-t', 'claude', () => app.fill('new-pane-dir', '~/code/ply'));
      expect(pane.cli).toBe('claude');
      await waitStatusLabel(j, pane.id, 'Your turn');
      await waitScreen(j, pane.id, `fake claude ${claudeSession(pane.id)} in ${realpathSync(dir)}`);
      expect(await app.textIn(`pane-${pane.id}-model`)).toBe('claude-example-model');
      expect(await app.textIn('tab-bar')).toContain('ply');
      expect(sb.launches('claude')).toHaveLength(1);
      expect(existsSync(join(sb.home, 'Library', 'LaunchAgents'))).toBe(false);
    },
    JOURNEY_MS,
  );

  test(
    'J2 parallel work: three agents and a shell; ⌘J cycles through what needs you',
    async () => {
      const j = await start();
      const a = await open(j, 'cmd-t');
      const b = await open(j, 'cmd-n', 'codex');
      const c = await open(j, 'cmd-n');
      const shell = await terminalHere(j);
      expect(shell.cli).toBe('shell');
      expect(await j.app.textIn('cli-counts')).toBe('2 claude · 1 codex · 1 sh');
      expect(await j.app.visiblePanes()).toEqual([a.id, b.id, c.id, shell.id]);
      for (const p of [a, c]) await waitStatusLabel(j, p.id, 'Your turn');

      const approvalMs = await paintedAfter(
        j,
        () => j.sb.fake(b.id).send('osc9 Approval requested: cargo test'),
        'Approval requested: cargo test',
      );
      const questionMs = await paintedAfter(
        j,
        () =>
          j.sb.fake(c.id).hook(
            'Notification',
            claudePayload(c, 'Notification', {
              message: 'Claude asks which branch to use',
              notification_type: 'elicitation_dialog',
            }),
          ),
        'Claude asks which branch to use',
      );
      console.log(
        `J2 F1 on the full app: needs-you strip painted ${approvalMs.toFixed(0)} ms (Codex OSC 9) and ${questionMs.toFixed(0)} ms (Claude hook) after the fake fired`,
      );
      expect(approvalMs).toBeLessThan(250);
      expect(questionMs).toBeLessThan(250);
      await j.app.waitForText('2 need you');
      await waitStatusLabel(j, b.id, 'Needs you');
      await waitStatusLabel(j, c.id, 'Needs you');

      await j.app.keys('cmd-j');
      await j.obs.waitFocus(b.id, '⌘J to reach the codex pane');
      await j.app.keys('cmd-j');
      await j.obs.waitFocus(c.id, '⌘J to reach the second claude pane');
      await j.app.keys('cmd-j');
      await j.obs.waitFocus(b.id, '⌘J to wrap around');

      await j.app.click(`answer-${b.id}-1`);
      await j.obs.waitPane(b.id, (p) => p.status === 'running', 'the answered pane to run');
      await until(async () => !(await j.app.has(`pane-${b.id}-waiting`)), 'the strip to go');
      await j.app.waitForText('1 needs you');
    },
    JOURNEY_MS,
  );

  test(
    'J3 many agents: two tabs of three agents; ⌘J jumps across tabs',
    async () => {
      const j = await start();
      const first = [
        await open(j, 'cmd-t'),
        await open(j, 'cmd-n', 'codex'),
        await open(j, 'cmd-n'),
      ];
      const second = [
        await open(j, 'cmd-t', 'codex'),
        await open(j, 'cmd-n'),
        await open(j, 'cmd-n'),
      ];
      const [a1, , a3] = first as [Pane, Pane, Pane];
      const [, b2] = second as [Pane, Pane, Pane];
      expect(new Set(first.map((p) => p.tab_id)).size).toBe(1);
      expect(new Set(second.map((p) => p.tab_id)).size).toBe(1);
      expect(a1.tab_id).not.toBe(b2.tab_id);
      expect(await j.app.visiblePanes()).toEqual(second.map((p) => p.id));

      await j.app.keys('cmd-1');
      await until(
        async () => (await j.app.visiblePanes()).join() === first.map((p) => p.id).join(),
        '⌘1 to show the first tab',
      );
      await j.obs.waitFocus(a3.id, 'the first tab to be active');
      await claudeAsks(j, b2);
      await j.obs.waitPane(b2.id, (p) => p.status === 'waiting_permission', 'the prompt');
      await j.app.waitForText('1 needs you');

      await j.app.keys('cmd-j');
      await j.obs.waitFocus(b2.id, '⌘J to jump to the second tab');
      expect(await j.app.visiblePanes()).toEqual(second.map((p) => p.id));
      await until(() => j.app.has(`pane-${b2.id}-waiting`), 'the needs-you strip');

      await claudeAsks(j, a1);
      await j.app.waitForText('2 need you');
      await j.app.keys('cmd-j');
      await j.obs.waitFocus(a1.id, '⌘J to jump back to the first tab');
      expect(await j.app.visiblePanes()).toEqual(first.map((p) => p.id));
      await j.app.keys('cmd-j');
      await j.obs.waitFocus(b2.id, '⌘J to cycle to the second tab again');
    },
    JOURNEY_MS,
  );

  test(
    'J4 terminal here: ⌘D on a claude pane in a worktree opens a shell in that worktree',
    async () => {
      const j = await start();
      const dir = repo(j.sb, 'ply');
      const claude = await open(j, 'cmd-t', 'claude', async () => {
        await j.app.fill('new-pane-dir', '~/code/ply');
        await j.app.click('new-pane-worktree');
        await until(() => j.app.has('new-pane-worktree-name'), 'the worktree name field');
        await j.app.fill('new-pane-worktree-name', 'feature-x');
      });
      const tree = realpathSync(join(dir, '.claude', 'worktrees', 'feature-x'));
      const moved = await j.obs.waitPane(
        claude.id,
        (p) => p.cwd === tree && p.worktree_seen === 'feature-x',
        'the claude pane to report its worktree',
      );
      expect(j.sb.launches('claude')[0]).toContain('--worktree feature-x');
      await until(
        async () => (await j.app.textIn(`pane-${claude.id}`)).includes('worktree feature-x'),
        'the worktree label in the header',
      );

      const shell = await terminalHere(j);
      expect(shell.cli).toBe('shell');
      expect(shell.cwd).toBe(moved.cwd);
      await j.obs.waitFocus(shell.id, 'the new shell to take focus');
      await until(
        async () => /[$#] ?$/.test((await j.app.textIn(`terminal-${shell.id}`)).trimEnd()),
        'the shell prompt',
      );
      await j.app.type('pwd -P\n');
      await waitScreen(j, shell.id, tree);
    },
    JOURNEY_MS,
  );

  test(
    'J5 quit and return: every agent keeps running and every pane shows its current screen',
    async () => {
      const j = await start();
      const claude = await open(j, 'cmd-t');
      const codex = await open(j, 'cmd-n', 'codex');
      const shell = await terminalHere(j);
      await j.sb.fake(claude.id).send('out beforequit1');
      await j.sb.fake(codex.id).send('out beforequit2');
      await until(
        async () => /[$#] ?$/.test((await j.app.textIn(`terminal-${shell.id}`)).trimEnd()),
        'the shell prompt',
      );
      await j.app.type('echo beforequit3; sleep 2; echo afterquit3\n');
      await waitScreen(j, claude.id, 'beforequit1');
      await waitScreen(j, codex.id, 'beforequit2');
      await waitScreen(j, shell.id, 'beforequit3');
      const plyd = j.sb.plydPid();
      const oldApp = j.app;

      expect(await oldApp.quit()).toBe('cmd-q');
      expect(alive(oldApp.pid)).toBe(false);
      await j.sb.fake(claude.id).send('out afterquit1');
      await j.sb.fake(codex.id).send('out afterquit2');
      await Bun.sleep(2_500);
      const kept = await j.obs.panes();
      expect(kept.map((p) => [p.id, p.status])).toEqual([
        [claude.id, 'idle'],
        [codex.id, 'idle'],
        [shell.id, 'idle'],
      ]);
      expect(j.sb.plydPid()).toBe(plyd);

      const app = await launch(j.sb);
      const back = { ...j, app };
      expect(await app.visiblePanes()).toEqual([claude.id, codex.id, shell.id]);
      await waitScreen(back, claude.id, 'afterquit1');
      await waitScreen(back, codex.id, 'afterquit2');
      await waitScreen(back, shell.id, 'afterquit3');
      for (const [id, before] of [
        [claude.id, 'beforequit1'],
        [codex.id, 'beforequit2'],
        [shell.id, 'beforequit3'],
      ] as const) {
        expect(await app.textIn(`terminal-${id}`)).toContain(before);
      }
      await waitStatusLabel(back, claude.id, 'Your turn');
      await waitStatusLabel(back, codex.id, 'Your turn');
      expect(j.sb.plydPid()).toBe(plyd);
      expect(j.sb.launches('claude')).toHaveLength(1);
      expect(j.sb.launches('codex')).toHaveLength(1);
    },
    JOURNEY_MS,
  );

  test(
    'J6 daemon restart: kill plyd; session panes show lost and resume, session-less panes reopen as shells',
    async () => {
      const j = await start();
      const claude = await open(j, 'cmd-t');
      const codex = await open(j, 'cmd-n', 'codex');
      const silent = await open(j, 'cmd-n', 'claude', () =>
        j.app.fill('new-pane-prompt', 'wait-for-start'),
      );
      const shell = await terminalHere(j);
      await j.sb.fake(codex.id).send('session');
      await j.obs.waitPane(claude.id, (p) => p.session_ref === claudeSession(claude.id), 'claude');
      await j.obs.waitPane(codex.id, (p) => p.session_ref === codexThread(codex.id), 'codex');
      expect((await j.obs.panes()).find((p) => p.id === silent.id)?.session_ref).toBeUndefined();
      const plyd = j.sb.plydPid() as number;

      process.kill(plyd, 'SIGKILL');
      await until(() => !alive(plyd), 'plyd to die');
      const restarted = await until(() => {
        const pid = j.sb.plydPid();
        return pid !== undefined && pid !== plyd ? pid : undefined;
      }, "the app's launcher to start plyd again");
      expect(restarted).not.toBe(plyd);
      await j.obs.waitPane(claude.id, (p) => p.status === 'lost', 'claude to be lost', 20_000);
      await j.obs.waitPane(codex.id, (p) => p.status === 'lost', 'codex to be lost');
      await j.obs.waitPane(
        silent.id,
        (p) => p.cli === 'shell',
        'the session-less agent pane as a shell',
      );
      await j.obs.waitPane(shell.id, (p) => p.status === 'idle', 'the shell reopened');
      for (const id of [claude.id, codex.id]) {
        await until(() => j.app.has(`resume-${id}`), `the Resume button of pane ${id}`);
        await waitStatusLabel(j, id, 'Lost');
      }
      for (const id of [silent.id, shell.id]) {
        expect(await j.app.has(`pane-${id}-lost`)).toBe(false);
        await until(
          async () => /[$#] ?$/.test((await j.app.textIn(`terminal-${id}`)).trimEnd()),
          `a fresh shell prompt in pane ${id}`,
        );
      }

      await j.app.click(`resume-${claude.id}`);
      await j.obs.waitPane(claude.id, (p) => p.status === 'idle', 'claude resumed');
      await waitStatusLabel(j, claude.id, 'Your turn');
      await waitScreen(j, claude.id, `fake claude ${claudeSession(claude.id)}`);
      expect(j.sb.launches('claude').at(-1)).toContain(`--resume ${claudeSession(claude.id)}`);
      await j.app.click(`resume-${codex.id}`);
      await j.obs.waitPane(codex.id, (p) => p.status === 'idle', 'codex resumed');
      await waitScreen(j, codex.id, `fake codex ${codexThread(codex.id)}`);
      expect(j.sb.launches('codex').at(-1)).toContain(`resume ${codexThread(codex.id)}`);
      expect(await j.app.has(`pane-${claude.id}-lost`)).toBe(false);

      const sessions = await j.obs.client.request('session.list', {
        workspace_id: j.obs.workspaceId,
        include_closed: true,
      });
      const byPane = new Map(sessions.map((s) => [s.pane_id, s]));
      expect(byPane.get(claude.id)?.session_ref).toBe(claudeSession(claude.id));
      expect(byPane.get(codex.id)?.session_ref).toBe(codexThread(codex.id));
      expect(byPane.get(silent.id)?.cli).toBe('shell');
      const log = readFileSync(
        join(j.sb.plyHome, 'run', 'panes', String(claude.id), 'launch.json'),
      );
      expect(JSON.parse(log.toString()).resume).toBe(claudeSession(claude.id));
    },
    JOURNEY_MS,
  );
});
