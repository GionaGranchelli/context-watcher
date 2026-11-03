import fs from "node:fs";
function trace(kind, event) { if (process.env.CW104B_TRACE_FILE) fs.appendFileSync(process.env.CW104B_TRACE_FILE, JSON.stringify({kind, keys:Object.keys(event || {}), payloadKeys:Object.keys(event?.payload || {}),messageShapes:(event?.payload?.messages||[]).map(m=>({role:m.role,type:typeof m.content,count:Array.isArray(m.content)?m.content.length:null,blocks:Array.isArray(m.content)?m.content.map(b=>({type:b.type,marker:typeof b.text==="string"&&b.text.includes("CW104B_FINAL_REQUEST_BOUNDARY")})):null,marker:typeof m.content==="string"&&m.content.includes("CW104B_FINAL_REQUEST_BOUNDARY")}))})+"\n"); }
// Experimental request metadata only; identical in both arms.
export default function requestOptions(pi) {
 trace("loaded", {});
 pi.on("context", async (event) => {trace("context",event);});
 pi.on("before_provider_request", async (event) => {
  trace("before_provider_request",event);
  const payload = structuredClone(event.payload);
  const marker = "CW104B_FINAL_REQUEST_BOUNDARY";
  const matches = [];
  for (let i = 0; i < (payload.messages || []).length; i++) {
   const c = payload.messages[i].content;
   const text = typeof c === "string" ? c : Array.isArray(c) && c.every(b => b.type === "text") ? c.map(b => b.text).join("\n") : null;
   if (text && text.includes(marker)) {
    if (text.indexOf(marker) !== text.lastIndexOf(marker)) throw new Error("Ambiguous prefix boundary");
    matches.push({ messages: i + 1 });
   }
  }
  if (matches.length !== 1 || matches[0].messages >= payload.messages.length) throw new Error("Missing or ambiguous whole-message prefix boundary");
  payload.strata_prefix = matches[0]; payload.seed = 104002;
  return payload;
 });
}
