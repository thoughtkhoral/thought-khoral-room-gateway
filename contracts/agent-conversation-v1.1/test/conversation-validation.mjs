// SPDX-License-Identifier: Apache-2.0
// Test-only semantic oracle. This is not a published/shared runtime library.
import { createHash } from 'node:crypto';

const PROFILE = 'thought-khoral.agent-conversation.v1';
const CODEX_ID = '74686f75-6768-746b-686f-72616c000004';
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const equal = (a, b) => canonicalBytes(a).equals(canonicalBytes(b));

function scalarString(value) {
  if (/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u.test(value)) {
    throw new Error('unicode-scalar');
  }
  return value;
}

export function canonicalBytes(value) {
  const encode = item => {
    if (item === null || typeof item === 'boolean') return JSON.stringify(item);
    if (typeof item === 'string') return JSON.stringify(scalarString(item));
    if (typeof item === 'number') {
      if (!Number.isSafeInteger(item) || item < 0 || Object.is(item, -0)) throw new Error('json-number');
      return String(item);
    }
    if (Array.isArray(item)) return '[' + item.map(encode).join(',') + ']';
    if (typeof item === 'object' && Object.getPrototypeOf(item) === Object.prototype) {
      const keys = Object.keys(item).sort();
      if (keys.some(key => !/^[\x20-\x7e]*$/.test(key))) throw new Error('json-key');
      return '{' + keys.map(key => JSON.stringify(key) + ':' + encode(item[key])).join(',') + '}';
    }
    throw new Error('json-value');
  };
  return Buffer.from(encode(value), 'utf8');
}

// JSON.parse loses duplicate keys and integer-token spelling. Parse the raw
// fixture before either loss can bypass the normative canonicalization rules.
export function parseStrictJson(text) {
  let offset = 0;
  const fail = () => { throw new Error('json-syntax'); };
  const whitespace = () => { while (/[\x20\t\r\n]/.test(text[offset] ?? '\0')) offset++; };
  const string = () => {
    const start = offset++;
    while (offset < text.length) {
      const char = text[offset++];
      if (char === '\\') { offset++; continue; }
      if (char === '"') {
        try { return scalarString(JSON.parse(text.slice(start, offset))); }
        catch (error) { if (error.message === 'unicode-scalar') throw error; fail(); }
      }
    }
    fail();
  };
  const value = () => {
    whitespace();
    if (text[offset] === '"') return string();
    if (text[offset] === '{') {
      offset++; whitespace(); const result = {}; const keys = new Set();
      if (text[offset] === '}') { offset++; return result; }
      while (true) {
        whitespace(); if (text[offset] !== '"') fail();
        const key = string();
        if (keys.has(key)) throw new Error('json-duplicate');
        keys.add(key); whitespace(); if (text[offset++] !== ':') fail();
        Object.defineProperty(result, key, { value: value(), enumerable: true, writable: true, configurable: true });
        whitespace(); const next = text[offset++];
        if (next === '}') return result;
        if (next !== ',') fail();
      }
    }
    if (text[offset] === '[') {
      offset++; whitespace(); const result = [];
      if (text[offset] === ']') { offset++; return result; }
      while (true) {
        result.push(value()); whitespace(); const next = text[offset++];
        if (next === ']') return result;
        if (next !== ',') fail();
      }
    }
    for (const [token, decoded] of [['true', true], ['false', false], ['null', null]]) {
      if (text.startsWith(token, offset)) { offset += token.length; return decoded; }
    }
    const match = /^-?(?:0|[1-9]\d*)(?:\.\d+)?(?:[eE][+-]?\d+)?/.exec(text.slice(offset));
    if (!match) fail();
    offset += match[0].length;
    const number = Number(match[0]);
    if (/[.eE-]/.test(match[0]) || !Number.isSafeInteger(number)) throw new Error('json-number');
    return number;
  };
  const result = value(); whitespace(); if (offset !== text.length) fail();
  return result;
}

function digestPreimage(packet) {
  const { digest, ...context } = packet.context;
  return { roomId: packet.roomId, agentId: packet.agentId,
    conversationId: packet.conversation.id, generation: packet.conversation.generation,
    triggerEventId: packet.triggerEventId, guidanceRevision: packet.guidanceRevision, context };
}

export function semanticErrors(schema, value, trusted = {}) {
  const errors = [];
  const check = (condition, code) => { if (!condition && !errors.includes(code)) errors.push(code); };
  const binding = (actual, key, code) => {
    if (trusted[key] !== undefined) check(actual === trusted[key], code);
  };
  const unique = values => new Set(values).size === values.length;
  const ordered = values => values.every((x, i) => i === 0 || x > values[i - 1]);
  const settings = setting => {
    if (setting?.confirmation === 'confirmed') check(setting.model !== null && setting.reasoningEffort !== null, 'settings-confirmation');
  };
  const usage = report => {
    if (!report) return;
    if (report.modelContextWindow === null) check(report.freshness === 'unavailable', 'usage-window');
    if (trusted.rawUsage) {
      check(report.lastTotalTokens === trusted.rawUsage.lastTotalTokens &&
        report.modelContextWindow === trusted.rawUsage.modelContextWindow, 'usage-counts');
    }
    if (trusted.rawUsageBinding) {
      const raw = trusted.rawUsageBinding;
      check(raw.taskId === trusted.taskId && raw.generation === trusted.generation &&
        raw.threadId === trusted.expectedThreadId && raw.turnId === trusted.expectedTurnId, 'usage-binding');
    }
  };
  const reply = output => {
    binding(output.conversationId, 'conversationId', 'conversation-binding');
    binding(output.generation, 'generation', 'generation-binding');
    binding(output.consumedRevision, 'contextRevision', 'revision-binding');
    binding(output.contextDigest, 'contextDigest', 'context-digest');
    check(Buffer.byteLength(output.assistantText, 'utf8') <= 65536, 'result-bytes');
    if (trusted.authorizedSourceIds) check(output.citations.every(id => trusted.authorizedSourceIds.includes(id)), 'citation-binding');
    settings(output.effectiveSettings); usage(output.usage);
  };
  const updateData = item => {
    if (item.kind === 'completed') reply(item.data);
    if (item.kind === 'settings') settings(item.data);
    if (item.kind === 'usage') usage(item.data);
  };

  if (value.profileVersion !== undefined) check(value.profileVersion === PROFILE, 'profile-binding');
  if (schema === 'resolved-settings') {
    binding(value.roomId, 'roomId', 'room-binding');
    binding(value.agentId, 'agentId', 'agent-binding');
    if (trusted.catalogRevision !== undefined) {
      check(value.selectedSettings.catalogRevision === trusted.catalogRevision, 'catalog-stale');
    }
    if (trusted.models !== undefined) {
      const model = trusted.models.find(model => model.id === value.selectedSettings.model);
      check(model !== undefined && model.efforts.includes(value.selectedSettings.reasoningEffort), 'unsupported-settings');
    }
  }
  if (schema === 'turn') {
    binding(value.roomId, 'roomId', 'room-binding'); binding(value.agentId, 'agentId', 'agent-binding');
    const ids = value.mentions.filter(m => m.type === 'participant').map(m => m.id);
    const aliases = value.mentions.filter(m => m.type === 'alias').map(m => m.alias);
    check(unique(ids) && unique(aliases), 'mention-duplicate');
    const direct = value.mentions.filter(m => m.type === 'participant' && m.id === value.agentId);
    const token = trusted.agentToken ?? (value.agentId === CODEX_ID ? 'codex-agent' : undefined);
    check(direct.length === 1 && (token === undefined || direct[0].token === token), 'direct-mention');
    if (value.conversation?.mode === 'continue') {
      binding(value.conversation.id, 'conversationId', 'conversation-binding');
      binding(value.conversation.generation, 'generation', 'generation-binding');
    }
    if (value.settings && trusted.catalogRevision) {
      check(value.settings.catalogRevision === trusted.catalogRevision, 'catalog-stale');
      const model = trusted.models?.find(m => m.id === value.settings.model);
      check(model !== undefined && model.efforts.includes(value.settings.reasoningEffort), 'unsupported-settings');
    }
    if (trusted.prior) check(trusted.sameRequester === true && equal(value, trusted.prior), 'duplicate-conflict');
  }
  if (schema === 'catalog') {
    check(unique(value.data.map(m => m.id)), 'catalog-duplicate');
    for (const model of value.data) {
      check(unique(model.supportedReasoningEfforts.map(e => e.id)), 'catalog-duplicate');
      check(model.defaultReasoningEffort === null || model.supportedReasoningEfforts.some(e => e.id === model.defaultReasoningEffort), 'catalog-default');
    }
  }
  if (schema === 'input') {
    const ctx = value.context, conv = value.conversation;
    for (const [key, code] of [['roomId','room-binding'],['agentId','agent-binding'],['taskId','task-binding'],['requesterId','requester-binding'],['guidanceRevision','guidance-binding']]) binding(value[key], key, code);
    binding(conv.id, 'conversationId', 'conversation-binding'); binding(conv.generation, 'generation', 'generation-binding');
    binding(ctx.baseRevision, 'baseRevision', 'context-base'); binding(ctx.policyRevision, 'policyRevision', 'policy-binding');
    binding(ctx.digest, 'contextDigest', 'context-digest'); binding(ctx.revision, 'contextRevision', 'revision-binding');
    binding(value.triggerEventId, 'triggerEventId', 'trigger-binding');
    check(ctx.revision > ctx.baseRevision, 'context-base');
    if (conv.mode === 'new') check(ctx.kind === 'baseline' && ctx.baseRevision === 0 && ctx.nativeReplyBindings.length === 0, 'baseline-binding');
    else check(ctx.kind === 'delta', 'context-base');
    const records = [...ctx.entries, ...ctx.nativeReplyBindings];
    check(records.length <= 2000, 'context-records');
    check(unique(records.map(e => e.eventId)) && unique(ctx.activeDecisions.map(d => d.decisionId)) &&
      unique([...records.map(e => e.eventId), ...ctx.activeDecisions.map(d => d.decisionId)]), 'source-duplicate');
    check(ordered(ctx.entries.map(e => e.sequence)) && ordered(ctx.nativeReplyBindings.map(e => e.sequence)) &&
      unique(records.map(e => e.sequence)) && records.every(e => e.sequence > ctx.baseRevision && e.sequence <= ctx.revision), 'sequence-order');
    const triggers = ctx.entries.filter(e => e.eventId === value.triggerEventId);
    check(triggers.length === 1 && triggers[0].authorRole === 'human' && triggers[0].authorId === value.requesterId && triggers[0].sequence === ctx.revision, 'trigger-binding');
    if (trusted.triggerText !== undefined) check(triggers.length === 1 && triggers[0].text === trusted.triggerText, 'trigger-binding');
    const sources = new Set(trusted.authorizedSourceIds ?? records.map(e => e.eventId));
    if (trusted.authorizedSourceIds) check(records.every(e => sources.has(e.eventId)) &&
      ctx.activeDecisions.every(d => sources.has(d.decisionId)), 'disclosure-binding');
    for (const decision of ctx.activeDecisions) check(decision.sourceEventIds.every(id => sources.has(id)), 'decision-provenance');
    if (trusted.roomWideSourceIds) check(records.every(e => trusted.roomWideSourceIds.includes(e.eventId)) && ctx.activeDecisions.every(d => trusted.roomWideSourceIds.includes(d.decisionId) && d.sourceEventIds.every(id => trusted.roomWideSourceIds.includes(id))), 'disclosure-binding');
    for (const native of ctx.nativeReplyBindings) {
      check(native.generation === conv.generation, 'generation-binding');
      check((trusted.acknowledgedReplies ?? []).some(ack => equal(ack, native)), 'native-ack-binding');
    }
    const bytes = canonicalBytes(digestPreimage(value));
    check(bytes.length <= 1048576, 'context-bytes'); check(hash(bytes) === ctx.digest, 'context-digest');
    const issued = Date.parse(value.issuedAt), expires = Date.parse(value.expiresAt);
    const auth = Date.parse(value.authorizationExpiresAt), lease = Date.parse(value.leaseExpiresAt);
    check(issued < expires && expires - issued <= 180000 && expires <= auth && expires <= lease, 'deadline-binding');
    if (trusted.now) {
      const now = Date.parse(trusted.now);
      check(now < expires && now < auth && now < lease, 'authority-expired');
      check(now >= issued, 'deadline-binding');
    }
    binding(value.leaseOwner, 'leaseOwner', 'lease-binding');
  }
  if (schema === 'result') reply(value);
  if (schema === 'view') {
    binding(value.roomId, 'roomId', 'room-binding'); binding(value.agentId, 'agentId', 'agent-binding');
  }
  if (schema === 'view' && value.conversation) {
    const conv = value.conversation;
    binding(conv.id, 'conversationId', 'conversation-binding'); binding(conv.generation, 'generation', 'generation-binding');
    check(['reserved','running'].includes(conv.state) ? conv.activeTaskId !== null : conv.activeTaskId === null, 'conversation-state');
    settings(conv.effectiveSettings); usage(conv.usage);
  }
  if (schema === 'update') {
    binding(value.taskId, 'taskId', 'task-binding'); binding(value.generation, 'generation', 'generation-binding');
    binding(value.contextDigest, 'contextDigest', 'context-digest'); updateData(value);
    if (value.kind === 'completed') {
      check(value.data.generation === value.generation, 'generation-binding');
      check(value.data.contextDigest === value.contextDigest, 'context-digest');
    }
    if (trusted.prior) check(equal(value, trusted.prior), 'terminal-conflict');
  }
  if (schema === 'task') {
    binding(value.roomId, 'roomId', 'room-binding'); binding(value.taskId, 'taskId', 'task-binding');
    binding(value.conversationId, 'conversationId', 'conversation-binding'); binding(value.generation, 'generation', 'generation-binding');
    if (value.state !== undefined) {
      binding(value.requesterId, 'requesterId', 'requester-binding');
      check(unique(value.updates.map(u => u.updateId)) && ordered(value.updates.map(u => u.ordinal)), 'update-order');
      for (const update of value.updates) updateData(update);
      settings(value.effectiveSettings); usage(value.usage);
      const terminal = value.updates.filter(u => ['completed','failed'].includes(u.kind));
      if (['completed','failed'].includes(value.state)) {
        check(terminal.length === 1 && terminal[0] === value.updates.at(-1) && terminal[0].kind === value.state, 'task-terminal');
        if (value.state === 'completed') {
          reply(value.result);
          check(value.result.conversationId === value.conversationId && value.result.generation === value.generation, 'task-terminal');
          check(terminal.length === 1 && equal(terminal[0].data, value.result), 'task-terminal');
        } else check(terminal.length === 1 && terminal[0].data.code === value.failure.code, 'task-terminal');
      } else check(terminal.length === 0, 'task-terminal');
    }
  }
  if (schema === 'ack') {
    if (trusted.prior) check(equal(value, trusted.prior), 'terminal-conflict');
    for (const [key, code] of [['taskId','task-binding'],['conversationId','conversation-binding'],['generation','generation-binding'],['contextDigest','context-digest'],['consumedRevision','revision-binding'],['replyEventId','reply-binding'],['replySequence','reply-binding']]) {
      if (key === 'consumedRevision') binding(value[key], 'contextRevision', code); else binding(value[key], key, code);
    }
    check(value.replySequence > value.consumedRevision, 'revision-binding');
    if (trusted.result) check(value.textDigest === hash(Buffer.from(trusted.result.assistantText, 'utf8')), 'text-digest');
  }
  return errors;
}
