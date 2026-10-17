import { Icon } from "../../app/Icon";

/** The `.library-tools` locate control (prototype `.tool-button.tool-icon`
 *  anatomy, like the sort control). */
export function LocateButton({ onClick }: { readonly onClick: () => void }) {
  return (
    <button
      type="button"
      className="tool-button tool-icon"
      aria-label="定位当前播放歌曲"
      title="定位当前播放歌曲"
      onClick={onClick}
      data-testid="locate-song"
    >
      <Icon name="locate" />
    </button>
  );
}
