export function selectEntries(entries, { only = null, shard = null } = {}) {
  const filtered = only ? entries.filter((entry) => entry.id.includes(only)) : entries;
  if (shard === null) return filtered;

  const match = /^(\d+)\/(\d+)$/.exec(shard);
  if (!match) throw new Error(`--shard must look like <i>/<n>, got '${shard}'`);
  const index = Number(match[1]);
  const total = Number(match[2]);
  if (total < 1 || index < 1 || index > total) {
    throw new Error(`--shard ${shard} is out of range`);
  }

  const buckets = Array.from({ length: total }, () => []);
  filtered.forEach((entry, position) => {
    const assigned = entry.shard ?? (position % total) + 1;
    if (assigned < 1 || assigned > total) {
      throw new Error(`entry '${entry.id}' declares shard ${assigned}, outside 1..${total}`);
    }
    buckets[assigned - 1].push(entry);
  });

  const union = buckets.flat();
  if (union.length !== filtered.length || new Set(union).size !== filtered.length) {
    throw new Error("injection shards must be disjoint and cover every selected entry");
  }
  for (let i = 0; i < total; i += 1) {
    if (buckets[i].length === 0) throw new Error(`injection shard ${i + 1}/${total} is empty`);
  }
  return buckets[index - 1];
}
