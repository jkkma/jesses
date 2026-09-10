import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const directory = mkdtempSync(join(tmpdir(), 'jesses-contracts-'));
try {
  const output = join(directory, 'generated.ts');
  execFileSync(
    'cargo',
    ['run', '--locked', '-p', 'media-core', '--example', 'export_types', '--', output],
    { stdio: 'inherit' },
  );
  if (readFileSync(output, 'utf8') !== readFileSync('src/lib/ipc/generated.ts', 'utf8')) {
    throw new Error(
      'IPC contracts have changed. Run pnpm contracts and review the generated changes.',
    );
  }
} finally {
  rmSync(directory, { recursive: true, force: true });
}
