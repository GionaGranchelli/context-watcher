function count(value: unknown): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) {
    throw new Error('Invalid strategy response count');
  }
  return value;
}

export function strategyConfirmation({ status, name, result }: {
  status: 'draft' | 'committed';
  name: string;
  result: unknown;
}): string {
  if (typeof result !== 'object' || result === null ||
      !('created_count' in result) || !('applied_count' in result) ||
      !('estimated_saved_tokens' in result)) {
    throw new Error('Incomplete strategy response');
  }
  const created = count(result.created_count);
  const preview = count(result.applied_count);
  const estimate = count(result.estimated_saved_tokens);
  const action = status === 'draft' ? 'Staged' : 'Created';
  return `${action} ${created} ${status} ${created === 1 ? 'rule' : 'rules'} from "${name}". ` +
    `${preview} eligible in the current preview. ` +
    `Estimated token reduction from created rules: ${estimate.toLocaleString()} tokens. ` +
    'Measured savings: unavailable.';
}
