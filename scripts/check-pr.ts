import { readFileSync } from 'node:fs';

/** Spec 0.1.3: the problems with a PR body, empty when it names a `Spec: x.y.z` and a `WP: n` line. */
export function checkPrBody(body: string): string[] {
  const problems: string[] = [];
  if (!/^\s*Spec:\s*\d+\.\d+\.\d+\s*$/m.test(body)) {
    problems.push('missing a `Spec: x.y.z` line naming the spec version the PR implements');
  }
  if (!/^\s*WP:\s*\d+(\s*[,+]\s*\d+)*\s*$/m.test(body)) {
    problems.push('missing a `WP: n` line naming the work package (several as `WP: 1, 2`)');
  }
  return problems;
}

function main(): void {
  const file = process.argv[2];
  const body = file ? readFileSync(file, 'utf8') : (process.env.PR_BODY ?? '');
  const problems = checkPrBody(body);
  for (const p of problems) console.log(`check-pr: ${p}`);
  if (problems.length > 0) process.exit(1);
  console.log('check-pr: ok');
}

if (import.meta.main) main();
