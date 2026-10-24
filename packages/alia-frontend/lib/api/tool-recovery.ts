import type { EffectRecord, ExecuteToolRequest, JsonValue } from './types';

function sameJson(left: JsonValue, right: JsonValue): boolean {
  if (left === right) return true;
  if (!left || !right || typeof left !== 'object' || typeof right !== 'object') return false;
  if (Array.isArray(left) || Array.isArray(right)) {
    return (
      Array.isArray(left) &&
      Array.isArray(right) &&
      left.length === right.length &&
      left.every((value, index) => sameJson(value, right[index]))
    );
  }
  const keys = Object.keys(left);
  return (
    keys.length === Object.keys(right).length &&
    keys.every((key) => Object.hasOwn(right, key) && sameJson(left[key], right[key]))
  );
}

// Recovery observes the old ledger; only a missing invocation may submit again.
// Current tool/policy checks still apply to proposal, confirmation and dispatch.
export async function executeToolRequest(
  request: ExecuteToolRequest,
  reconcile: boolean,
  readPage: (before?: string) => Promise<EffectRecord[]>,
  submit: () => Promise<EffectRecord>
): Promise<EffectRecord> {
  if (reconcile) {
    let before: string | undefined;
    while (true) {
      const records = await readPage(before);
      const existing = records.find((record) => record.key.invocation_key === '0');
      if (existing) {
        if (
          existing.key.request_id !== request.request_id ||
          existing.tool_name !== request.tool_name ||
          !sameJson(existing.arguments, request.arguments)
        ) {
          throw new Error('This request is already recorded with different tool arguments.');
        }
        return existing;
      }
      if (records.length < 100) break;
      const next = records.at(-1)!.effect_id;
      if (next === before)
        throw new Error('Execution recovery did not advance. Retry status lookup.');
      before = next;
    }
  }
  return submit();
}
