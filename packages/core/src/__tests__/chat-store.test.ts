import { describe, it, expect, beforeEach } from "vitest";
import { useChatStore } from "../stores/chat-store.js";

// Reset store state before each test
beforeEach(() => {
  useChatStore.getState().reset();
});

describe("useChatStore", () => {
  describe("initial state", () => {
    it("starts with a single welcome message", () => {
      const msgs = useChatStore.getState().messages;
      expect(msgs).toHaveLength(1);
      expect(msgs[0]!.id).toBe("welcome");
      expect(msgs[0]!.role).toBe("assistant");
    });

    it("starts with open: true", () => {
      expect(useChatStore.getState().open).toBe(true);
    });

    it("starts with streaming: false", () => {
      expect(useChatStore.getState().streaming).toBe(false);
    });

    it("starts with error: null", () => {
      expect(useChatStore.getState().error).toBeNull();
    });
  });

  describe("setOpen / toggleOpen", () => {
    it("setOpen sets the open flag", () => {
      useChatStore.getState().setOpen(true);
      expect(useChatStore.getState().open).toBe(true);
      useChatStore.getState().setOpen(false);
      expect(useChatStore.getState().open).toBe(false);
    });

    it("toggleOpen flips the flag", () => {
      // reset() leaves open: true
      useChatStore.getState().toggleOpen();
      expect(useChatStore.getState().open).toBe(false);
      useChatStore.getState().toggleOpen();
      expect(useChatStore.getState().open).toBe(true);
    });
  });

  describe("addUserMessage", () => {
    it("adds a user message after the welcome", () => {
      useChatStore.getState().addUserMessage("hello");
      const msgs = useChatStore.getState().messages;
      expect(msgs).toHaveLength(2);
      expect(msgs[1]!.role).toBe("user");
      expect(msgs[1]!.content).toBe("hello");
    });

    it("assigns a unique id and timestamp", () => {
      useChatStore.getState().addUserMessage("a");
      useChatStore.getState().addUserMessage("b");
      const msgs = useChatStore.getState().messages;
      // msgs[0] is welcome, 1 and 2 are the new user messages
      expect(msgs[1]!.id).toBeTruthy();
      expect(msgs[2]!.id).toBeTruthy();
      expect(msgs[1]!.id).not.toBe(msgs[2]!.id);
      expect(typeof msgs[1]!.timestamp).toBe("number");
    });

    it("attaches context pills when provided", () => {
      const ctx = [{ partId: "p1", partName: "Box", geometryType: "part" as const }];
      useChatStore.getState().addUserMessage("select this", ctx);
      const msgs = useChatStore.getState().messages;
      expect(msgs[msgs.length - 1]!.context).toEqual(ctx);
    });
  });

  describe("addAssistantMessage", () => {
    it("adds an assistant message", () => {
      useChatStore.getState().addAssistantMessage("Hi there");
      const msgs = useChatStore.getState().messages;
      expect(msgs).toHaveLength(2);
      expect(msgs[1]!.role).toBe("assistant");
      expect(msgs[1]!.content).toBe("Hi there");
    });

    it("attaches toolCalls when provided", () => {
      const toolCalls = [{ id: "tc1", name: "create_box", args: {}, status: "pending" as const }];
      useChatStore.getState().addAssistantMessage("done", toolCalls);
      const msgs = useChatStore.getState().messages;
      expect(msgs[msgs.length - 1]!.toolCalls).toEqual(toolCalls);
    });
  });

  describe("updateLastAssistant", () => {
    it("updates the content of the last assistant message", () => {
      useChatStore.getState().addAssistantMessage("partial");
      useChatStore.getState().updateLastAssistant("full response");
      const msgs = useChatStore.getState().messages;
      expect(msgs[msgs.length - 1]!.content).toBe("full response");
    });

    it("does not modify earlier messages", () => {
      useChatStore.getState().addUserMessage("q");
      useChatStore.getState().addAssistantMessage("partial");
      useChatStore.getState().updateLastAssistant("updated");
      const msgs = useChatStore.getState().messages;
      // msgs[0] = welcome, msgs[1] = user "q", msgs[2] = assistant "updated"
      expect(msgs[1]!.content).toBe("q");
    });

    it("is a no-op when last message is not from assistant", () => {
      useChatStore.getState().addUserMessage("q");
      useChatStore.getState().updateLastAssistant("should not apply");
      const msgs = useChatStore.getState().messages;
      expect(msgs[msgs.length - 1]!.content).toBe("q");
    });
  });

  describe("setStreaming", () => {
    it("sets streaming flag", () => {
      useChatStore.getState().setStreaming(true);
      expect(useChatStore.getState().streaming).toBe(true);
      useChatStore.getState().setStreaming(false);
      expect(useChatStore.getState().streaming).toBe(false);
    });
  });

  describe("setError", () => {
    it("sets error string", () => {
      useChatStore.getState().setError("something went wrong");
      expect(useChatStore.getState().error).toBe("something went wrong");
    });

    it("clears error with null", () => {
      useChatStore.getState().setError("err");
      useChatStore.getState().setError(null);
      expect(useChatStore.getState().error).toBeNull();
    });
  });

  describe("clearThread", () => {
    it("clears messages back to welcome and resets streaming/error", () => {
      useChatStore.getState().addUserMessage("hi");
      useChatStore.getState().setStreaming(true);
      useChatStore.getState().setError("oops");
      useChatStore.getState().clearThread();
      const s = useChatStore.getState();
      expect(s.messages).toHaveLength(1);
      expect(s.messages[0]!.id).toBe("welcome");
      expect(s.streaming).toBe(false);
      expect(s.error).toBeNull();
    });

    it("preserves open state", () => {
      useChatStore.getState().setOpen(true);
      useChatStore.getState().clearThread();
      expect(useChatStore.getState().open).toBe(true);
    });
  });

  describe("reset", () => {
    it("resets everything to initial state (welcome message, open, no stream/error)", () => {
      useChatStore.getState().setOpen(false);
      useChatStore.getState().addUserMessage("hi");
      useChatStore.getState().setStreaming(true);
      useChatStore.getState().setError("err");
      useChatStore.getState().reset();
      const s = useChatStore.getState();
      expect(s.messages).toHaveLength(1);
      expect(s.messages[0]!.id).toBe("welcome");
      expect(s.open).toBe(true);
      expect(s.streaming).toBe(false);
      expect(s.error).toBeNull();
    });
  });

  describe("anonUsage / usageError", () => {
    it("initial anonUsage is zero with token-based limit", () => {
      const s = useChatStore.getState();
      expect(s.anonUsage.used).toBe(0);
      expect(s.anonUsage.limit).toBeGreaterThan(100);
    });

    it("addAnonTokens accumulates the counter", () => {
      useChatStore.getState().addAnonTokens(1500);
      expect(useChatStore.getState().anonUsage.used).toBe(1500);
      useChatStore.getState().addAnonTokens(2300);
      expect(useChatStore.getState().anonUsage.used).toBe(3800);
    });

    it("setAnonUsage replaces the counter with an absolute value", () => {
      useChatStore.getState().setAnonUsage(7000);
      expect(useChatStore.getState().anonUsage.used).toBe(7000);
      useChatStore.getState().setAnonUsage(2000);
      expect(useChatStore.getState().anonUsage.used).toBe(2000);
    });

    it("setUsageError / clear on reset", () => {
      useChatStore.getState().setUsageError({
        kind: "anon_limit",
        message: "hi",
        limit: 3,
      });
      expect(useChatStore.getState().usageError?.kind).toBe("anon_limit");
      useChatStore.getState().reset();
      expect(useChatStore.getState().usageError).toBeNull();
    });

    it("clearThread clears usageError", () => {
      useChatStore.getState().setUsageError({
        kind: "monthly_limit",
        message: "hi",
      });
      useChatStore.getState().clearThread();
      expect(useChatStore.getState().usageError).toBeNull();
    });
  });
});
