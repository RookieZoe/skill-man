import { Buffer } from "node:buffer";
import { createHash, createPublicKey, verify } from "node:crypto";
import { createReadStream } from "node:fs";
import { readFile } from "node:fs/promises";

// Tauri wraps Minisign public keys and detached signatures in base64.
// Verify both the archive signature and its authenticated trusted comment.
export async function verifyUpdaterSignature(
  archivePath,
  signature,
  publicKey,
) {
  try {
    const keyLines = decode(publicKey)
      .toString("utf8")
      .trimEnd()
      .split(/\r?\n/);
    const sigLines = decode(signature)
      .toString("utf8")
      .trimEnd()
      .split(/\r?\n/);
    if (
      keyLines.length !== 2 ||
      sigLines.length !== 4 ||
      !sigLines[2].startsWith("trusted comment: ")
    ) {
      throw new Error("invalid Minisign envelope");
    }
    const key = decode(keyLines[1]);
    const sig = decode(sigLines[1]);
    const globalSignature = decode(sigLines[3]);
    if (
      key.length !== 42 ||
      key.subarray(0, 2).toString() !== "Ed" ||
      sig.length !== 74 ||
      globalSignature.length !== 64 ||
      !key.subarray(2, 10).equals(sig.subarray(2, 10))
    ) {
      throw new Error("invalid key or signature identity");
    }
    const algorithm = sig.subarray(0, 2).toString();
    let message;
    if (algorithm === "ED") {
      const hash = createHash("blake2b512");
      for await (const chunk of createReadStream(archivePath))
        hash.update(chunk);
      message = hash.digest();
    } else if (algorithm === "Ed") {
      // Tauri also accepts legacy, non-prehashed Minisign signatures.
      message = await readFile(archivePath);
    } else {
      throw new Error("unsupported signature algorithm");
    }
    const ed25519Key = createPublicKey({
      key: Buffer.concat([
        Buffer.from("302a300506032b6570032100", "hex"),
        key.subarray(10),
      ]),
      format: "der",
      type: "spki",
    });
    const detached = sig.subarray(10);
    const comment = Buffer.from(sigLines[2].slice("trusted comment: ".length));
    if (
      !verify(null, message, ed25519Key, detached) ||
      !verify(
        null,
        Buffer.concat([detached, comment]),
        ed25519Key,
        globalSignature,
      )
    ) {
      throw new Error("signature verification failed");
    }
  } catch (error) {
    throw new Error(`updater signature verification failed: ${error.message}`, {
      cause: error,
    });
  }
}

function decode(value) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length % 4 !== 0 ||
    !/^[A-Za-z0-9+/]+={0,2}$/.test(value)
  ) {
    throw new Error("invalid base64 encoding");
  }
  const bytes = Buffer.from(value, "base64");
  if (bytes.toString("base64") !== value)
    throw new Error("noncanonical base64 encoding");
  return bytes;
}
