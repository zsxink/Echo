import { SORT_FIELDS, type SongSort } from "./types";
import { SortMenu } from "./SortMenu";

interface SongSortControlProps {
  readonly sort: SongSort;
  readonly onChange: (sort: SongSort) => void;
  readonly fields?: ReadonlyArray<{ value: SongSort["field"]; label: string }>;
}

/** The shared compact sort menu used by manually sortable song views. */
export function SongSortControl({ sort, onChange, fields = SORT_FIELDS }: SongSortControlProps) {
  return <SortMenu sort={sort} options={fields} onChange={onChange} />;
}
