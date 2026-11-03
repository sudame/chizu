import { describe, expect, it } from "vitest";
import { type EscapeState, type EscapeStep, nextEscapeStep } from "./escape";

describe("nextEscapeStep", () => {
  it("引きかけ、浮いたパネル、道具、検査の欄の順に、1 段ずつ戻る", () => {
    const all: EscapeState = {
      drawing: true,
      popoverOpen: true,
      toolSelected: true,
      inspectorOpen: true,
    };
    expect(nextEscapeStep(all)).toBe("cancel-drawing");
    expect(nextEscapeStep({ ...all, drawing: false })).toBe("close-popover");
    expect(nextEscapeStep({ ...all, drawing: false, popoverOpen: false })).toBe("select-view");
    expect(
      nextEscapeStep({
        drawing: false,
        popoverOpen: false,
        toolSelected: false,
        inspectorOpen: true,
      }),
    ).toBe("close-inspector");
    expect(
      nextEscapeStep({
        drawing: false,
        popoverOpen: false,
        toolSelected: false,
        inspectorOpen: false,
      }),
    ).toBe("nothing");
  });

  it("どの状態から始めても、押し続ければ「見る」を経て何も残らない", () => {
    for (let bits = 0; bits < 16; bits++) {
      let state: EscapeState = {
        drawing: (bits & 1) !== 0,
        popoverOpen: (bits & 2) !== 0,
        toolSelected: (bits & 4) !== 0,
        inspectorOpen: (bits & 8) !== 0,
      };
      const seen: EscapeStep[] = [];
      for (let i = 0; i < 4; i++) {
        const step = nextEscapeStep(state);
        seen.push(step);
        state = {
          drawing: step === "cancel-drawing" ? false : state.drawing,
          popoverOpen: step === "close-popover" ? false : state.popoverOpen,
          toolSelected: step === "select-view" ? false : state.toolSelected,
          inspectorOpen: step === "close-inspector" ? false : state.inspectorOpen,
        };
      }
      expect(state.toolSelected).toBe(false);
      expect(nextEscapeStep(state)).toBe("nothing");
      if ((bits & 4) !== 0) {
        expect(seen).toContain("select-view");
      }
    }
  });
});
