import { test, mock, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { createReconnectingSocket, reconnectDelayMs } from './reconnecting-socket.ts';

const CONNECTING = 0;
const OPEN = 1;
const CLOSING = 2;
const CLOSED = 3;

class FakeSocket {
  constructor(url) {
    this.url = url;
    this.readyState = CONNECTING;
    this.sent = [];
    this.closeCalls = 0;
    this.onopen = null;
    this.onmessage = null;
    this.onclose = null;
    this.onerror = null;
  }
  send(data) { this.sent.push(JSON.parse(data)); }
  close() { this.closeCalls += 1; if (this.readyState !== CLOSED) this.readyState = CLOSING; }
  // Test drivers.
  open() { this.readyState = OPEN; this.onopen?.({}); }
  receive(data) { this.onmessage?.({ data }); }
  serverClose(code = 1006) { this.readyState = CLOSED; this.onclose?.({ code }); }
}

let timersEnabled = false;
function enableTimers() {
  if (!timersEnabled) {
    mock.timers.enable({ apis: ['setTimeout', 'setInterval'] });
    timersEnabled = true;
  }
}
afterEach(() => {
  mock.timers.reset();
  timersEnabled = false;
});

function setup(overrides = {}) {
  enableTimers();
  const sockets = [];
  const statuses = [];
  const events = { disconnects: 0, errors: 0, messages: [] };
  const controller = createReconnectingSocket({
    url: 'ws://test/ws/positions',
    authToken: 'jwt-token',
    baseDelayMs: 3000,
    maxDelayMs: 30_000,
    pingIntervalMs: 30_000,
    random: () => 0, // deterministic: delay = ceiling / 2
    createSocket: (url) => { const socket = new FakeSocket(url); sockets.push(socket); return socket; },
    onStatus: (status) => statuses.push(status),
    onMessage: (data) => events.messages.push(data),
    onDisconnect: () => { events.disconnects += 1; },
    onError: () => { events.errors += 1; },
    ...overrides,
  });
  return { controller, sockets, statuses, events };
}

test('reconnectDelayMs grows exponentially, is capped, and stays within the jitter band', () => {
  const at = (attempt, random) => reconnectDelayMs(attempt, { baseMs: 3000, maxMs: 30_000, random: () => random });
  // Ceilings: 3s, 6s, 12s, 24s, then capped at 30s. Jitter keeps the delay in [ceiling/2, ceiling).
  assert.deepEqual([0, 1, 2, 3, 4, 5, 50, 5000].map((n) => at(n, 0)), [1500, 3000, 6000, 12_000, 15_000, 15_000, 15_000, 15_000]);
  assert.deepEqual([0, 1, 2, 3, 4, 5].map((n) => at(n, 0.999999)), [3000, 6000, 12_000, 24_000, 30_000, 30_000]);
  for (let attempt = 0; attempt < 40; attempt += 1) {
    for (const random of [0, 0.25, 0.5, 0.75, 0.999999]) {
      const delay = at(attempt, random);
      assert.ok(Number.isFinite(delay), `attempt ${attempt}`);
      assert.ok(delay <= 30_000 && delay >= 1500, `attempt ${attempt} -> ${delay}`);
    }
  }
  // Real randomness stays inside the band as well.
  const real = reconnectDelayMs(2, { baseMs: 3000, maxMs: 30_000 });
  assert.ok(real >= 6000 && real <= 12_000);
});

test('connect opens one socket, authenticates on open and pings on the interval', () => {
  const { controller, sockets, statuses } = setup();
  controller.connect();
  assert.equal(sockets.length, 1);
  assert.deepEqual(statuses, ['connecting']);
  assert.equal(sockets[0].url, 'ws://test/ws/positions');

  sockets[0].open();
  assert.deepEqual(sockets[0].sent, [{ action: 'authenticate', token: 'jwt-token' }]);
  mock.timers.tick(30_000);
  assert.deepEqual(sockets[0].sent.at(-1), { action: 'ping' });
  assert.equal(sockets[0].sent.filter((frame) => frame.action === 'ping').length, 1);
  mock.timers.tick(60_000);
  assert.equal(sockets[0].sent.filter((frame) => frame.action === 'ping').length, 3);
});

test('a public channel sends no authenticate frame', () => {
  const { controller, sockets } = setup({ authToken: null });
  controller.connect();
  sockets[0].open();
  assert.deepEqual(sockets[0].sent, []);
});

test('a token provider is asked at every open, and a failure closes the socket to retry', async () => {
  let issued = 0;
  let fail = false;
  const { controller, sockets } = setup({
    authToken: async () => {
      if (fail) throw new Error('401');
      issued += 1;
      return `stream-${issued}`;
    },
  });
  controller.connect();
  sockets[0].open();
  await Promise.resolve();
  assert.deepEqual(sockets[0].sent, [{ action: 'authenticate', token: 'stream-1' }]);

  // The next connection gets a fresh token.
  sockets[0].serverClose(1006);
  mock.timers.tick(1500);
  sockets[1].open();
  await Promise.resolve();
  assert.deepEqual(sockets[1].sent, [{ action: 'authenticate', token: 'stream-2' }]);

  // No token: nothing is sent, the socket is closed (and onclose retries).
  sockets[1].serverClose(1006);
  fail = true;
  mock.timers.tick(3000); // the second retry's backoff
  sockets[2].open();
  await Promise.resolve();
  await Promise.resolve();
  assert.deepEqual(sockets[2].sent.filter((frame) => frame.action === 'authenticate'), []);
  assert.equal(sockets[2].closeCalls, 1);
});

test('regression: connect() while a socket is still CONNECTING does not create a duplicate', () => {
  const { controller, sockets } = setup();
  controller.connect();
  controller.connect();
  controller.resume(); // tab visibility handler
  assert.equal(sockets.length, 1);
  assert.equal(sockets[0].closeCalls, 0);

  // Once open, still nothing to replace.
  sockets[0].open();
  controller.connect();
  controller.resume();
  assert.equal(sockets.length, 1);
});

test('reconnects forever with exponential, capped backoff (no five-attempt limit)', () => {
  const { controller, sockets } = setup();
  controller.connect();
  const expectedDelays = [1500, 3000, 6000, 12_000, 15_000, 15_000, 15_000, 15_000, 15_000, 15_000, 15_000, 15_000];
  for (let i = 0; i < expectedDelays.length; i += 1) {
    const before = sockets.length;
    sockets.at(-1).serverClose(1006);
    mock.timers.tick(expectedDelays[i] - 1);
    assert.equal(sockets.length, before, `attempt ${i}: reconnected too early`);
    mock.timers.tick(1);
    assert.equal(sockets.length, before + 1, `attempt ${i}: did not reconnect after ${expectedDelays[i]}ms`);
  }
  // Far beyond the old limit of five attempts.
  assert.ok(sockets.length > 10);
});

test('backoff resets after a confirmed connection', () => {
  const { controller, sockets, statuses } = setup();
  controller.connect();
  for (let i = 0; i < 4; i += 1) {
    sockets.at(-1).serverClose(1006);
    mock.timers.tick(30_000);
  }
  assert.equal(sockets.length, 5);

  sockets.at(-1).open();
  controller.markConnected();
  assert.equal(statuses.at(-1), 'connected');

  const before = sockets.length;
  sockets.at(-1).serverClose(1006);
  mock.timers.tick(1499);
  assert.equal(sockets.length, before);
  mock.timers.tick(1);
  assert.equal(sockets.length, before + 1, 'delay is back to the first-attempt delay');
});

test('a policy-violation close (rejected token) reports an error and does not loop', () => {
  const { controller, sockets, statuses, events } = setup();
  controller.connect();
  sockets[0].open();
  sockets[0].serverClose(1008);
  assert.equal(statuses.at(-1), 'error');
  assert.equal(events.disconnects, 1);
  mock.timers.tick(10 * 60_000);
  assert.equal(sockets.length, 1);

  // An explicit resume (for example the tab becoming visible) tries again.
  controller.resume();
  assert.equal(sockets.length, 2);
});

test('a CLOSED leftover is released, with its ping timer, before the replacement opens', () => {
  const { controller, sockets } = setup();
  controller.connect();
  sockets[0].open();
  const stale = sockets[0];

  // The browser dropped the socket without delivering a close event we handled.
  stale.readyState = CLOSED;
  controller.resume();

  assert.equal(sockets.length, 2);
  assert.equal(stale.closeCalls, 1);
  assert.equal(stale.onopen, null);
  assert.equal(stale.onmessage, null);
  assert.equal(stale.onclose, null);
  assert.equal(stale.onerror, null);

  sockets[1].open();
  const staleFramesBefore = stale.sent.length;
  mock.timers.tick(30_000);
  // Only the replacement's interval is alive: one ping, nothing from the old socket.
  assert.equal(stale.sent.length, staleFramesBefore);
  assert.equal(sockets[1].sent.filter((frame) => frame.action === 'ping').length, 1);
});

test('events from a superseded socket cannot reschedule a reconnect or change status', () => {
  const { controller, sockets, statuses, events } = setup();
  controller.connect();
  const first = sockets[0];
  const lateClose = first.onclose;
  const lateMessage = first.onmessage;
  const lateError = first.onerror;
  const lateOpen = first.onopen;

  controller.reconnectNow();
  assert.equal(sockets.length, 2);
  sockets[1].open();
  const statusCount = statuses.length;

  lateClose({ code: 1006 });
  lateMessage({ data: 'stale' });
  lateError({});
  lateOpen({});
  mock.timers.tick(5 * 60_000);

  assert.equal(sockets.length, 2, 'no reconnect scheduled by the stale close');
  assert.equal(statuses.length, statusCount);
  assert.equal(events.disconnects, 0);
  assert.equal(events.errors, 0);
  assert.deepEqual(events.messages, []);
});

test('ping timers are never leaked across reconnect cycles', () => {
  const { controller, sockets } = setup();
  controller.connect();
  for (let cycle = 0; cycle < 5; cycle += 1) {
    sockets.at(-1).open();
    sockets.at(-1).serverClose(1006);
    mock.timers.tick(30_000);
  }
  const last = sockets.at(-1);
  last.open();
  for (const socket of sockets.slice(0, -1)) socket.sent.length = 0;
  last.sent.length = 0;
  mock.timers.tick(30_000);
  assert.equal(last.sent.filter((frame) => frame.action === 'ping').length, 1);
  for (const socket of sockets.slice(0, -1)) {
    assert.equal(socket.sent.length, 0, 'a closed socket must not be pinged');
  }
});

test('messages from the current socket are forwarded; disconnect() stops reconnecting', () => {
  const { controller, sockets, statuses, events } = setup();
  controller.connect();
  sockets[0].open();
  sockets[0].receive('{"type":"Pong"}');
  assert.deepEqual(events.messages, ['{"type":"Pong"}']);

  controller.disconnect();
  assert.equal(sockets[0].closeCalls, 1);
  sockets[0].serverClose(1000);
  assert.equal(statuses.at(-1), 'disconnected');
  mock.timers.tick(10 * 60_000);
  assert.equal(sockets.length, 1, 'an explicit disconnect never reconnects on its own');

  controller.connect();
  assert.equal(sockets.length, 2);
});

test('reconnect: false never schedules a retry', () => {
  const { controller, sockets } = setup({ reconnect: false });
  controller.connect();
  sockets[0].serverClose(1006);
  mock.timers.tick(10 * 60_000);
  assert.equal(sockets.length, 1);
});

test('maxAttempts bounds consecutive retries when an owner opts in', () => {
  const { controller, sockets } = setup({ maxAttempts: 2 });
  controller.connect();
  for (let i = 0; i < 6; i += 1) {
    sockets.at(-1).serverClose(1006);
    mock.timers.tick(60_000);
  }
  assert.equal(sockets.length, 3); // the first connection plus two retries
});

test('resume() while backoff is pending reconnects immediately and cancels the timer', () => {
  const { controller, sockets } = setup();
  controller.connect();
  sockets[0].serverClose(1006);
  controller.resume();
  assert.equal(sockets.length, 2);
  mock.timers.tick(60_000);
  assert.equal(sockets.length, 2, 'the cancelled backoff timer must not open another socket');
});

test('reconnectNow() replaces the socket immediately and resets backoff', () => {
  const { controller, sockets } = setup();
  controller.connect();
  for (let i = 0; i < 3; i += 1) {
    sockets.at(-1).serverClose(1006);
    mock.timers.tick(30_000);
  }
  const previous = sockets.at(-1);
  previous.open();
  controller.reconnectNow();
  assert.equal(previous.closeCalls, 1);
  assert.equal(sockets.length, 5);
  sockets.at(-1).serverClose(1006);
  mock.timers.tick(1500);
  assert.equal(sockets.length, 6, 'first-attempt delay after reconnectNow');
});

test('dispose() closes the socket, clears every timer and ignores later calls', () => {
  const { controller, sockets, statuses, events } = setup();
  controller.connect();
  sockets[0].open();
  const statusCount = statuses.length;
  controller.dispose();

  assert.equal(sockets[0].closeCalls, 1);
  assert.equal(sockets[0].onclose, null);
  mock.timers.tick(10 * 60_000);
  assert.equal(sockets[0].sent.filter((frame) => frame.action === 'ping').length, 0, 'ping timer cleared');

  // Reconnect pending at dispose time never fires.
  const second = setup();
  second.controller.connect();
  second.sockets[0].serverClose(1006);
  second.controller.dispose();
  mock.timers.tick(10 * 60_000);
  assert.equal(second.sockets.length, 1);

  controller.connect();
  controller.resume();
  controller.reconnectNow();
  assert.equal(sockets.length, 1);
  assert.equal(statuses.length, statusCount);
  assert.equal(events.disconnects, 0);
  assert.equal(controller.send('{}'), false);
});

test('send only writes to an open socket', () => {
  const { controller, sockets } = setup();
  assert.equal(controller.send('{"a":1}'), false);
  controller.connect();
  assert.equal(controller.send('{"a":1}'), false);
  sockets[0].open();
  assert.equal(controller.send('{"a":1}'), true);
  assert.deepEqual(sockets[0].sent.at(-1), { a: 1 });
});

test('a socket constructor that throws reports an error and is not retried', () => {
  const statuses = [];
  enableTimers();
  const original = console.error;
  console.error = () => {};
  try {
    const controller = createReconnectingSocket({
      url: 'not a url',
      createSocket: () => { throw new SyntaxError('bad url'); },
      onStatus: (status) => statuses.push(status),
    });
    controller.connect();
    mock.timers.tick(10 * 60_000);
  } finally {
    console.error = original;
  }
  assert.deepEqual(statuses, ['connecting', 'error']);
});
