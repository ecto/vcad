import { customAlphabet } from "nanoid";

// 12 chars from a base62 alphabet — ~71 bits of entropy, far more than
// enough for per-user local document IDs, and much shorter/prettier than
// UUIDs in URLs like /~aB3kZ9pQwR2z.
const generate = customAlphabet(
  "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
  12,
);

export function newDocId(): string {
  return generate();
}
