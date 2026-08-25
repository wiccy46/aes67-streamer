import assert from "node:assert/strict";
import { $, browser } from "@wdio/globals";

async function getRoutingSnapshot() {
  return browser.tauri.execute(({ core }) => core.invoke("get_routing_snapshot"));
}

async function getRuntimeSnapshot() {
  return browser.tauri.execute(({ core }) => core.invoke("get_runtime_snapshot"));
}

async function waitForSnapshot(predicate, timeoutMsg) {
  let current;
  await browser.waitUntil(
    async () => {
      current = await getRoutingSnapshot();
      return predicate(current);
    },
    { timeoutMsg },
  );
  return current;
}

async function clickSelector(selector) {
  await browser.execute((targetSelector) => {
    const element = document.querySelector(targetSelector);
    if (!(element instanceof HTMLElement || element instanceof SVGElement)) {
      throw new Error(`cannot click missing element: ${targetSelector}`);
    }
    element.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
  }, selector);
}

async function waitForSelector(selector) {
  await browser.waitUntil(
    () => browser.execute((targetSelector) => Boolean(document.querySelector(targetSelector)), selector),
    { timeoutMsg: `UI action target did not appear: ${selector}` },
  );
}

async function setNumberInput(selector, value) {
  await waitForSelector(selector);
  await browser.execute(
    (targetSelector, nextValue) => {
      const input = document.querySelector(targetSelector);
      if (!(input instanceof HTMLInputElement)) {
        throw new Error(`cannot edit missing number input: ${targetSelector}`);
      }
      const valueSetter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
      input.focus();
      valueSetter?.call(input, String(nextValue));
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
      input.blur();
    },
    selector,
    value,
  );
}

describe("AES67 native routing workspace", () => {
  it("uses UI actions to mutate the authoritative Rust routing model", async () => {
    const engineStatus = await $('[data-testid="engine-status"]');
    await engineStatus.waitForDisplayed();

    const initial = await getRoutingSnapshot();
    assert.equal(initial.sources.length, 3);
    assert.equal(initial.streams.length, 3);
    assert.equal(initial.routes.length, 3);

    await clickSelector('[data-testid="add-source"]');
    const withSource = await waitForSnapshot(
      (snapshot) => snapshot.sources.length === 4,
      "Rust did not create the source requested by the UI",
    );
    assert.ok(withSource.revision > initial.revision);
    assert.equal(withSource.sources.at(-1).config.name, "Source 4");

    await clickSelector('[data-testid="add-stream"]');
    const withStream = await waitForSnapshot(
      (snapshot) => snapshot.streams.length === 4,
      "Rust did not create the stream requested by the UI",
    );
    assert.ok(withStream.revision > withSource.revision);
    assert.equal(withStream.streams.at(-1).config.address, "239.69.83.4");

    await setNumberInput('[data-testid="stream-3-gain"]', -18);
    const attenuated = await waitForSnapshot(
      (snapshot) => snapshot.streams.find((stream) => stream.id === 3)?.config.gain_db === -18,
      "Rust did not persist the per-stream gain requested by the UI",
    );
    assert.ok(attenuated.revision > withStream.revision);

    const sourceOutput = await $('[data-testid="source-1-output"]');
    const streamInput = await $('[data-testid="stream-3-input"]');
    await sourceOutput.dragAndDrop(streamInput);
    const reassigned = await waitForSnapshot(
      (snapshot) =>
        snapshot.routes.some((route) => route.source_id === 1 && route.stream_id === 3),
      "Rust did not apply the route reassignment requested by the UI",
    );
    assert.equal(
      reassigned.routes.some((route) => route.source_id === 2 && route.stream_id === 3),
      false,
    );
    assert.equal(reassigned.routes.length, 3);
    assert.equal(reassigned.streams.find((stream) => stream.id === 1).config.gain_db, 0);
    assert.equal(reassigned.streams.find((stream) => stream.id === 3).config.gain_db, -18);

    await setNumberInput('[data-testid="stream-3-gain"]', -121);
    const muted = await waitForSnapshot(
      (snapshot) => snapshot.streams.find((stream) => stream.id === 3)?.config.gain_db === null,
      "Rust did not normalize gain below -120 dB to mute",
    );
    assert.ok(muted.revision > reassigned.revision);

    const reassignedRouteSelector =
      '.react-flow__edge[data-id="source-1-stream-3"] .react-flow__edge-path';
    await waitForSelector(reassignedRouteSelector);
    await clickSelector(reassignedRouteSelector);
    await browser.keys("Delete");
    const removed = await waitForSnapshot(
      (snapshot) => !snapshot.routes.some((route) => route.stream_id === 3),
      "Rust did not remove the route deleted by the UI",
    );
    assert.equal(removed.routes.length, 2);

    await clickSelector('[data-testid="stream-1-sdp"]');
    await waitForSelector('[data-testid="sdp-dialog"]');
    const sdp = await browser.execute(
      () => document.querySelector('[data-testid="sdp-dialog"] pre')?.textContent ?? "",
    );
    assert.match(sdp, /m=audio 5004 RTP\/AVP 97/);
    assert.match(sdp, /a=rtpmap:97 L24\/48000\/2/);
    await clickSelector('[aria-label="Close SDP"]');

    await clickSelector('[data-testid="start-all"]');
    let liveRuntime;
    await browser.waitUntil(
      async () => {
        liveRuntime = await getRuntimeSnapshot();
        return (
          liveRuntime.lifecycle === "running" &&
          liveRuntime.streams.length === 2 &&
          liveRuntime.streams.every((stream) => stream.packets_sent > 0)
        );
      },
      { timeout: 15_000, timeoutMsg: "Rust runtime did not send packets for all routed streams" },
    );
    assert.ok(liveRuntime.streams.every((stream) => stream.lifecycle === "live"));
    assert.ok(liveRuntime.streams.every((stream) => stream.sdp.includes("a=sendonly")));

    await clickSelector('[data-testid="start-all"]');
    await browser.waitUntil(
      async () => (await getRuntimeSnapshot()).lifecycle === "stopped",
      { timeoutMsg: "Rust runtime did not stop all streams" },
    );
  });
});
