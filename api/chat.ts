import type { VercelRequest, VercelResponse } from "@vercel/node";
import { streamText } from "ai";

const SYSTEM_PROMPT = `You are vcad's AI assistant — a parametric CAD copilot embedded in a web-based CAD application.

You can both answer questions about CAD design and execute operations on the user's model.

Coordinate system: Z-up (X right, Y forward, Z up). Units: millimeters.

When the user asks you to modify geometry:
1. Use the available tools to execute the operation
2. Briefly confirm what you did after the tool call completes
3. If a tool call fails, explain the error and suggest alternatives

When the user asks questions:
- Be concise and practical
- Reference specific parts by name when relevant
- If you need more context about their model, ask

Context pills in user messages indicate what geometry is currently selected in the viewport. Use this context to understand which parts the user is referring to.`;

export default async function handler(req: VercelRequest, res: VercelResponse) {
  // CORS
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "POST, OPTIONS");
  res.setHeader("Access-Control-Allow-Headers", "Content-Type, Authorization");

  if (req.method === "OPTIONS") {
    res.status(200).end();
    return;
  }

  if (req.method !== "POST") {
    res.status(405).json({ error: "Method not allowed" });
    return;
  }

  const { messages, context } = req.body as {
    messages: Array<{ role: "user" | "assistant"; content: string }>;
    context?: { selectedParts: Array<{ partId: string; partName: string; geometryType: string }> };
  };

  if (!messages?.length) {
    res.status(400).json({ error: "messages required" });
    return;
  }

  let systemPrompt = SYSTEM_PROMPT;
  if (context?.selectedParts?.length) {
    const partList = context.selectedParts
      .map((p) => `- ${p.partName} (${p.geometryType}, id: ${p.partId})`)
      .join("\n");
    systemPrompt += `\n\nCurrently selected geometry:\n${partList}`;
  }

  try {
    const result = streamText({
      model: "anthropic/claude-sonnet-4.6",
      system: systemPrompt,
      messages,
      tools: {},
    });

    return result.toTextStreamResponse();
  } catch (err) {
    console.error("Chat API error:", err);
    res.status(500).json({ error: "Internal server error" });
  }
}
