import { describe, it, expect, beforeEach } from "vitest";
import { useChatStore } from "../stores/chat-store.js";

// Reset store state before each test
beforeEach(() => {
  useChatStore.getState().reset();
});

describe("useChatStore", () => {
  describe("initial state", () => {
    it("starts with empty messages", () => {
      expect(useChatStore.getState().messages).toEqual([]);
    });

    it("starts with open: false", () => {
      expect(useChatStore.getState().open).toBe(false);
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
      useChatStore.getState().toggleOpen();
      expect(useChatStore.getState().open).toBe(true);
      useChatStore.getState().toggleOpen();
      expect(useChatStore.getState().open).toBe(false);
    });
  });

  describe("addUserMessage", () => {
    it("adds a user message with role and content", () => {
      useChatStore.getState().addUserMessage("hello");
      const msgs = useChatStore.getState().messages;
      expect(msgs).toHaveLength(1);
      expect(msgs[0].role).toBe("user");
      expect(msgs[0].content).toBe("hello");
    });

    it("assigns a unique id and timestamp", () => {
      useChatStore.getState().addUserMessage("a");
      useChatStore.getState().addUserMessage("b");
      const msgs = useChatStore.getState().messages;
      expect(msgs[0].id).toBeTruthy();
      expect(msgs[1].id).toBeTruthy();
      expect(msgs[0].id).not.toBe(msgs[1].id);
      expect(typeof msgs[0].timestamp).toBe("number");
    });

    it("attaches context pills when provided", () => {
      const ctx = [{ partId: "p1", partName: "Box", geometryType: "part" as const }];
      useChatStore.getState().addUserMessage("select this", ctx);
      expect(useChatStore.getState().messages[0].context).toEqual(ctx);
    });
  });

  describe("addAssistantMessage", () => {
    it("adds an assistant message", () => {
      useChatStore.getState().addAssistantMessage("Hi there");
      const msgs = useChatStore.getState().messages;
      expect(msgs).toHaveLength(1);
      expect(msgs[0].role).toBe("assistant");
      expect(msgs[0].content).toBe("Hi there");
    });

    it("attaches toolCalls when provided", () => {
      const toolCalls = [{ id: "tc1", name: "create_box", args: {}, status: "pending" as const }];
      useChatStore.getState().addAssistantMessage("done", toolCalls);
      expect(useChatStore.getState().messages[0].toolCalls).toEqual(toolCalls);
    });
  });

  describe("updateLastAssistant", () => {
    it("updates the content of the last assistant message", () => {
      useChatStore.getState().addAssistantMessage("partial");
      useChatStore.getState().updateLastAssistant("full response");
      const msgs = useChatStore.getState().messages;
      expect(msgs[msgs.length - 1].content).toBe("full response");
    });

    it("does not modify earlier messages", () => {
      useChatStore.getState().addUserMessage("q");
      useChatStore.getState().addAssistantMessage("partial");
      useChatStore.getState().updateLastAssistant("updated");
      expect(useChatStore.getState().messages[0].content).toBe("q");
    });

    it("is a no-op when last message is not from assistant", () => {
      useChatStore.getState().addUserMessage("q");
      useChatStore.getState().updateLastAssistant("should not apply");
      expect(useChatStore.getState().messages[0].content).toBe("q");
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
    it("clears messages and resets streaming/error", () => {
      useChatStore.getState().addUserMessage("hi");
      useChatStore.getState().setStreaming(true);
      useChatStore.getState().setError("oops");
      useChatStore.getState().clearThread();
      const s = useChatStore.getState();
      expect(s.messages).toEqual([]);
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
    it("resets everything to initial state", () => {
      useChatStore.getState().setOpen(true);
      useChatStore.getState().addUserMessage("hi");
      useChatStore.getState().setStreaming(true);
      useChatStore.getState().setError("err");
      useChatStore.getState().reset();
      const s = useChatStore.getState();
      expect(s.messages).toEqual([]);
      expect(s.open).toBe(false);
      expect(s.streaming).toBe(false);
      expect(s.error).toBeNull();
    });
  });
});
