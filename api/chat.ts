import type { VercelRequest, VercelResponse } from "@vercel/node";
import { streamText } from "ai";

const FALLBACK_SYSTEM_PROMPT = "You are vcad's AI assistant — a parametric CAD copilot. Coordinate system: Z-up (X right, Y forward, Z up). Units: millimeters. Be concise.";

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

  const { messages, context, tools: clientTools, systemPrompt: clientSystemPrompt } = req.body as {
    messages: Array<{ role: "user" | "assistant"; content: string | object[] }>;
    context?: { selectedParts: Array<{ partId: string; partName: string; geometryType: string }> };
    tools?: Array<{ name: string; description: string; input_schema: Record<string, unknown> }>;
    systemPrompt?: string;
  };

  if (!messages?.length) {
    res.status(400).json({ error: "messages required" });
    return;
  }

  const systemPrompt = clientSystemPrompt || FALLBACK_SYSTEM_PROMPT;

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
