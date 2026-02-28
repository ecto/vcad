import type { VercelRequest, VercelResponse } from "@vercel/node";
import { createClient } from "@supabase/supabase-js";

// Auth client (anon key) for verifying user tokens
const supabase = createClient(
  process.env.SUPABASE_URL!,
  process.env.SUPABASE_ANON_KEY!
);

// Service role client for inserting logs (bypasses RLS)
const supabaseAdmin = createClient(
  process.env.SUPABASE_URL!,
  process.env.SUPABASE_SERVICE_ROLE_KEY!
);

// HuggingFace Inference Endpoint for cad0 model
const HF_ENDPOINT = process.env.HF_INFERENCE_ENDPOINT!;
const HF_TOKEN = process.env.HF_TOKEN!;

/**
 * Log an inference attempt to the database.
 * Returns the log ID if successful, undefined otherwise.
 */
async function logInference(params: {
  userId: string;
  prompt: string;
  result?: string;
  tokens?: number;
  durationMs?: number;
  error?: string;
}): Promise<string | undefined> {
  try {
    const { data } = await supabaseAdmin
      .from("inference_logs")
      .insert({
        user_id: params.userId,
        prompt: params.prompt,
        result: params.result ?? null,
        tokens: params.tokens ?? null,
        duration_ms: params.durationMs ?? null,
        error: params.error ?? null,
      })
      .select("id")
      .single();
    return data?.id;
  } catch (e) {
    // Don't fail the request if logging fails
    console.error("Failed to log inference:", e);
    return undefined;
  }
}

// System prompt for cad0 model
const SYSTEM_PROMPT =
  "You are a CAD assistant. Output only VCode code (C for box, Y for cylinder, T for translate, U for union, D for difference). No explanations, just the IR code.";

/**
 * Format prompt using Qwen chat template.
 */
function formatChatPrompt(userPrompt: string): string {
  return `<|im_start|>system
${SYSTEM_PROMPT}<|im_end|>
<|im_start|>user
${userPrompt}<|im_end|>
<|im_start|>assistant
`;
}

/**
 * Clean up generated IR text by removing markdown and extra content.
 */
function cleanGeneratedIR(text: string): string {
  let ir = text.trim();

  // Remove markdown code blocks
  if (ir.startsWith("```")) {
    ir = ir.replace(/^```(?:ir|text|plaintext)?\n?/, "").replace(/\n?```$/, "");
  }

  // Stop at common hallucination patterns and chat markers
  const stopPatterns = [
    "\n\n",
    "User",
    "user",
    "Now:",
    "Assistant",
    "Design:",
    "<|im_end|>",
    "<|im_start|>",
  ];
  for (const pattern of stopPatterns) {
    const idx = ir.indexOf(pattern);
    if (idx > 0) {
      ir = ir.substring(0, idx);
    }
  }

  // Find valid IR lines only
  const lines = ir.split("\n");
  const validOpcodes = [
    "C",
    "Y",
    "S",
    "K",
    "T",
    "R",
    "X",
    "U",
    "D",
    "I",
    "LP",
    "CP",
    "SH",
    "FI",
    "CH",
    "SK",
    "L",
    "A",
    "E",
    "V",
    "M",
    "ROOT",
    "PDEF",
    "INST",
    "END",
  ];

  // Minimum args required for each opcode
  const minArgs: Record<string, number> = {
    C: 3,
    Y: 2,
    S: 1,
    K: 3,
    T: 4,
    R: 4,
    X: 4,
    U: 2,
    D: 2,
    I: 2,
    SH: 2,
    FI: 2,
    CH: 2,
    LP: 4,
    CP: 4,
    SK: 1,
    L: 4,
    A: 7,
    E: 2,
    V: 2,
    M: 4,
    ROOT: 1,
    PDEF: 1,
    INST: 2,
    END: 0,
  };

  const validLines: string[] = [];
  for (const line of lines) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) {
      continue; // Skip empty lines and comments
    }
    const parts = trimmed.split(/\s+/);
    const opcode = parts[0] ?? "";
    if (!validOpcodes.includes(opcode)) {
      // Stop at first invalid opcode
      break;
    }
    // Check if line has enough arguments
    const required = minArgs[opcode] ?? 0;
    if (parts.length < required + 1) {
      // Incomplete line - skip it (likely truncated)
      break;
    }
    validLines.push(line);
  }

  return validLines.join("\n").trim();
}

// Allowed origins for CORS
const ALLOWED_ORIGINS = [
  "https://vcad.io",
  "https://www.vcad.io",
  "http://localhost:5173",
  "http://localhost:3000",
];

function setCorsHeaders(req: VercelRequest, res: VercelResponse): void {
  const origin = req.headers.origin;
  if (origin && ALLOWED_ORIGINS.includes(origin)) {
    res.setHeader("Access-Control-Allow-Origin", origin);
  }
  res.setHeader("Access-Control-Allow-Methods", "POST, OPTIONS");
  res.setHeader(
    "Access-Control-Allow-Headers",
    "Authorization, Content-Type"
  );
}

export default async function handler(
  req: VercelRequest,
  res: VercelResponse
): Promise<void> {
  // CORS headers
  setCorsHeaders(req, res);

  if (req.method === "OPTIONS") {
    res.status(200).end();
    return;
  }

  if (req.method !== "POST") {
    res.status(405).json({ error: "Method not allowed" });
    return;
  }

  // Verify auth via Supabase JWT
  const authHeader = req.headers.authorization;
  if (!authHeader?.startsWith("Bearer ")) {
    res.status(401).json({ error: "Unauthorized" });
    return;
  }

  const token = authHeader.slice(7);
  const {
    data: { user },
    error: authError,
  } = await supabase.auth.getUser(token);

  if (authError || !user) {
    res.status(401).json({ error: "Unauthorized" });
    return;
  }

  const { prompt } = req.body;
  if (!prompt || typeof prompt !== "string") {
    res.status(400).json({ error: "Prompt required" });
    return;
  }

  if (prompt.length > 2000) {
    res.status(400).json({ error: "Prompt too long (max 2000 characters)" });
    return;
  }

  // Check HF config
  if (!HF_ENDPOINT || !HF_TOKEN) {
    res.status(500).json({ error: "HF inference not configured" });
    return;
  }

  const startTime = Date.now();

  try {
    const response = await fetch(HF_ENDPOINT, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${HF_TOKEN}`,
      },
      body: JSON.stringify({
        inputs: formatChatPrompt(prompt),
        parameters: {
          max_new_tokens: 512,
          temperature: 0.1,
          do_sample: true,
          return_full_text: false,
        },
      }),
    });

    if (!response.ok) {
      const error = await response.text();
      throw new Error(`HF inference failed: ${error}`);
    }

    const result = (await response.json()) as
      | Array<{ generated_text?: string }>
      | { generated_text?: string; text?: string };

    // HF Inference Endpoint returns array for text-generation
    // or object with generated_text for TGI
    let generatedText: string;
    if (Array.isArray(result)) {
      generatedText = result[0]?.generated_text ?? "";
    } else {
      generatedText = result.generated_text ?? result.text ?? "";
    }

    // Clean up the generated IR
    const ir = cleanGeneratedIR(generatedText);
    const durationMs = Date.now() - startTime;

    // Log successful inference
    const logId = await logInference({
      userId: user.id,
      prompt,
      result: ir,
      tokens: generatedText.length,
      durationMs,
    });

    res.status(200).json({
      ir,
      tokens: generatedText.length, // Approximate
      durationMs,
      logId,
    });
  } catch (error) {
    const durationMs = Date.now() - startTime;
    const errorMsg = error instanceof Error ? error.message : "AI inference failed";

    // Log failed inference
    await logInference({
      userId: user.id,
      prompt,
      durationMs,
      error: errorMsg,
    });

    console.error("AI inference failed:", error);
    res.status(500).json({ error: errorMsg });
  }
}
