// SPDX-License-Identifier: Apache-2.0
import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import Ajv2020 from 'ajv/dist/2020.js';
import addFormats from 'ajv-formats';
import { parseStrictJson, semanticErrors } from './conversation-validation.mjs';

const loadText = path => readFile(new URL(`../${path}`, import.meta.url), 'utf8');
const load = async path => parseStrictJson(await loadText(path));
const ajv = new Ajv2020({ allErrors: true, strict: true });
addFormats(ajv);
for (const name of ['turn', 'view', 'catalog', 'task', 'input', 'update', 'result', 'error', 'ack', 'resolved-settings']) {
  ajv.addSchema(await load(`schemas/agent-conversation-v1/${name}.schema.json`));
}
const validate = ajv.getSchema('https://github.com/thoughtkhoral/thought-khoral-contracts/schemas/agent-conversation-v1/resolved-settings.schema.json');
const prefix = 'fixtures/agent-conversation-v1.1/';
const cases = await load(prefix + 'manifest.json');
const seen = new Set();
for (const fixture of cases) {
  assert.equal(fixture.schema, 'resolved-settings');
  assert.match(fixture.path, /^(valid|invalid)\/[a-z0-9-]+\.json$/);
  assert.ok(!seen.has(fixture.path), `duplicate fixture ${fixture.path}`);
  seen.add(fixture.path);
  let errors = [], value;
  try { value = parseStrictJson(await loadText(prefix + fixture.path)); }
  catch (error) { errors.push(error.message); }
  if (!errors.length) {
    if (!validate(value)) errors.push('schema: ' + ajv.errorsText(validate.errors));
    else errors.push(...semanticErrors(fixture.schema, value, fixture.context ?? {}));
  }
  assert.equal(errors.length === 0, fixture.valid, `${fixture.path}: ${errors.join('; ') || 'unexpectedly accepted'}`);
  if (!fixture.valid) assert.ok(errors.some(error => error.startsWith(fixture.reason)), `${fixture.path}: expected ${fixture.reason}, got ${errors.join('; ')}`);
}
for (const directory of ['valid', 'invalid']) {
  for (const file of await readdir(new URL(`../${prefix}${directory}/`, import.meta.url))) {
    assert.ok(seen.has(`${directory}/${file}`), `unlisted fixture ${directory}/${file}`);
  }
}
assert.ok(cases.some(f => f.valid) && cases.some(f => !f.valid));
const value = await load(prefix + 'valid/resolved-settings.json');
assert.deepEqual(semanticErrors('resolved-settings', value, {
  roomId: value.roomId, agentId: value.agentId, catalogRevision: 'catalog-1',
  models: [{ id: 'model-a', efforts: ['effort-medium'] }, { id: 'model-b', efforts: ['effort-high'] }],
}), []);
assert.ok(semanticErrors('resolved-settings', value, { catalogRevision: 'catalog-2', models: [] }).includes('catalog-stale'));
assert.ok(semanticErrors('resolved-settings', value, { models: [] }).includes('unsupported-settings'));
console.log(`validated ${cases.length} additive defaults schema, raw JSON and semantic fixtures`);
