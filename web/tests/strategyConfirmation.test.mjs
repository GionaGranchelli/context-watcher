import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

const source = await readFile(new URL('../src/lib/strategyConfirmation.ts', import.meta.url), 'utf8');
const { outputText } = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext } });
const { strategyConfirmation } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);

test('draft strategy reports created rules even when none apply in preview', () => {
  assert.equal(strategyConfirmation({ status: 'draft', name: 'Log diet', result: {
    created_count: 1, applied_count: 0, estimated_saved_tokens: 128,
  } }), 'Staged 1 draft rule from "Log diet". 0 eligible in the current preview. Estimated token reduction from created rules: 128 tokens. Measured savings: unavailable.');
});

test('committed inactive PinContext is created without claiming application or measured savings', () => {
  assert.equal(strategyConfirmation({ status: 'committed', name: 'Rule reminder', result: {
    created_count: 1, applied_count: 0, estimated_saved_tokens: 0,
  } }), 'Created 1 committed rule from "Rule reminder". 0 eligible in the current preview. Estimated token reduction from created rules: 0 tokens. Measured savings: unavailable.');
});

test('preview eligibility is distinct from creation and measured savings', () => {
  assert.equal(strategyConfirmation({ status: 'committed', name: 'Log diet', result: {
    created_count: 3, applied_count: 2, estimated_saved_tokens: 128,
  } }), 'Created 3 committed rules from "Log diet". 2 eligible in the current preview. Estimated token reduction from created rules: 128 tokens. Measured savings: unavailable.');
});

test('missing or invalid counts cannot silently become zero', () => {
  for (const result of [null, {}, { applied_count: 1, estimated_saved_tokens: 0 },
    { created_count: 1, applied_count: -1, estimated_saved_tokens: 0 },
    { created_count: '1', applied_count: 0, estimated_saved_tokens: 0 }]) {
    assert.throws(() => strategyConfirmation({ status: 'draft', name: 'Test', result }));
  }
});
