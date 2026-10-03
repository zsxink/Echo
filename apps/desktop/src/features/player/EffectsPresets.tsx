import type { EffectsSnapshotDto } from "../../ipc/ipc-types.generated";
import type { EffectsDialogAction } from "./EffectsDialog";

export function EffectsPresets({
  effects,
  onSelect,
  onManage,
}: {
  readonly effects: EffectsSnapshotDto;
  readonly onSelect: (id: string) => void;
  readonly onManage: (action: EffectsDialogAction) => void;
}) {
  const selection = effects.document.selection;
  return (
    <div className="effects-presets">
      {(["builtin", "user"] as const).map((source) => (
        <section key={source} aria-label={source === "builtin" ? "内置音效" : "我的音效"}>
          <h3>{source === "builtin" ? "内置音效" : "我的音效"}</h3>
          <div className="effects-preset-list">
            {effects.presets
              .filter((preset) => preset.source === source)
              .map((preset) => {
                const selected = selection.kind === "preset" && selection.id === preset.id;
                return (
                  <div
                    className={`effects-preset-row${selected ? " selected" : ""}`}
                    key={preset.id}
                  >
                    <button
                      className="effects-preset"
                      type="button"
                      aria-pressed={selected}
                      onClick={() => onSelect(preset.id)}
                    >
                      <span className="effects-preset-name">{preset.name}</span>
                      {source === "user" ? (
                        <small className="effects-custom-tag">自定义</small>
                      ) : null}
                    </button>
                    {source === "user" ? (
                      <div className="effects-preset-tools">
                        <button
                          type="button"
                          onClick={() =>
                            onManage({ kind: "rename", id: preset.id, name: preset.name })
                          }
                          aria-label={`重命名 ${preset.name}`}
                        >
                          重命名
                        </button>
                        <button
                          type="button"
                          onClick={() =>
                            onManage({ kind: "delete", id: preset.id, name: preset.name })
                          }
                          aria-label={`删除 ${preset.name}`}
                        >
                          删除
                        </button>
                      </div>
                    ) : null}
                  </div>
                );
              })}
          </div>
          {source === "user" && effects.document.userPresets.length === 0 ? (
            <p className="effects-note">在均衡器中编辑并保存曲线，便可在这里选用。</p>
          ) : null}
        </section>
      ))}
    </div>
  );
}
