import { createHash } from 'node:crypto';
import { lstatSync, readdirSync, readFileSync, readlinkSync } from 'node:fs';
import { join } from 'node:path';

/** Files inside the vendor tree that ply itself owns, so they are left out of the content hash. */
export const VENDOR_OWN_FILES = new Set(['vendor.json', 'patches.md']);

function collect(dir: string, prefix: string, out: string[]): void {
  for (const name of readdirSync(dir).sort()) {
    if (name === '.DS_Store') continue;
    const rel = prefix ? `${prefix}/${name}` : name;
    if (!prefix && VENDOR_OWN_FILES.has(name)) continue;
    const abs = join(dir, name);
    const st = lstatSync(abs);
    if (st.isSymbolicLink()) {
      const target = createHash('sha256').update(readlinkSync(abs)).digest('hex');
      out.push(`link ${target} ${rel}`);
    } else if (st.isDirectory()) {
      collect(abs, rel, out);
    } else if (st.isFile()) {
      const sum = createHash('sha256').update(readFileSync(abs)).digest('hex');
      const mode = st.mode & 0o111 ? 'exec' : 'file';
      out.push(`${mode} ${sum} ${rel}`);
    }
  }
}

/** SHA-256 over every file, symlink and exec bit under `dir`, git-ignored files included (ADR-0008). */
export function vendorContentHash(dir: string): { sha256: string; files: number } {
  const entries: string[] = [];
  collect(dir, '', entries);
  entries.sort((a, b) => {
    const pa = a.slice(a.indexOf(' ', 5) + 1);
    const pb = b.slice(b.indexOf(' ', 5) + 1);
    return pa < pb ? -1 : pa > pb ? 1 : 0;
  });
  const sha256 = createHash('sha256').update(entries.join('\n')).digest('hex');
  return { sha256, files: entries.length };
}
