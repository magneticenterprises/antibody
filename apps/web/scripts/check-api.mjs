#!/usr/bin/env node
// Contract drift check: regenerate the API types from the committed OpenAPI
// document into a temporary file and fail when they differ from the committed
// src/api/schema.d.ts. Fix drift with `pnpm run gen:api`.
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { generate, outputPath } from './api-codegen.mjs';

const dir = mkdtempSync(join(tmpdir(), 'antibody-api-check-'));
try {
  const tmp = join(dir, 'schema.d.ts');
  writeFileSync(tmp, await generate(), 'utf8');
  const expected = readFileSync(tmp, 'utf8');
  let actual;
  try {
    actual = readFileSync(outputPath, 'utf8');
  } catch {
    console.error('check:api: src/api/schema.d.ts is missing. Run `pnpm run gen:api`.');
    process.exit(1);
  }
  if (expected !== actual) {
    const a = actual.split('\n');
    const e = expected.split('\n');
    const line = e.findIndex((l, i) => l !== a[i]);
    console.error('check:api: src/api/schema.d.ts is out of date with openapi/openapi.json (contract drift).');
    console.error(`First difference at line ${line + 1}:`);
    console.error(`  committed: ${JSON.stringify(a[line] ?? '<end of file>')}`);
    console.error(`  generated: ${JSON.stringify(e[line] ?? '<end of file>')}`);
    console.error('Run `pnpm run gen:api`, then fix the type errors it causes.');
    process.exit(1);
  }
  console.log('check:api: src/api/schema.d.ts matches openapi/openapi.json');
} finally {
  rmSync(dir, { recursive: true, force: true });
}
