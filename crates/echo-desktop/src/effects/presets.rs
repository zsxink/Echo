//! Versioned engineering starting curves; these are not universal genre recipes.

use super::{EqCurve, Payload, PreampMode, PresetId, Spatial};
use serde::{Deserialize, Serialize};

pub const BUILTIN_IDS: [&str; 10] = [
    "pop",
    "rock",
    "classical",
    "jazz",
    "electronic",
    "vocal",
    "bass",
    "warm",
    "retro",
    "surround",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: PresetId,
    pub name: String,
    pub description: String,
    pub payload: Payload,
    pub source: PresetSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PresetSource {
    Builtin,
    User,
}

#[must_use]
pub fn builtin_presets() -> Vec<Preset> {
    let curves = [
        (
            "pop",
            "流行",
            "轻度突出低频和明亮感",
            [1., 2., 1., -1., -0.5, 0., 1., 2., 1., 0.],
        ),
        (
            "rock",
            "摇滚",
            "突出低频和乐器存在感",
            [3., 4., 2., 0., -1.5, -2., -0.5, 2., 3., 2.],
        ),
        (
            "classical",
            "古典",
            "接近中性，局部轻度提亮",
            [0., 0., 0., 0., 0., 0., 0., 0.5, 0.5, 0.5],
        ),
        (
            "jazz",
            "爵士",
            "增加厚度与轻度明亮感",
            [1., 1.5, 2., 1.5, 0.5, 0., 0.5, 1., 1.5, 1.],
        ),
        (
            "electronic",
            "电子",
            "加强节奏低频和明亮感",
            [2., 3., 2., -1., -1.5, -1., 0., 1.5, 2., 1.],
        ),
        (
            "vocal",
            "清澈人声",
            "调整整曲音色与存在感，也会影响乐器",
            [-1.5, -1.5, -1., -0.5, 0., 1., 2., 1., -1., 0.],
        ),
        (
            "bass",
            "超重低音",
            "突出低频厚度与力度",
            [1.5, 4., 3., 0.5, 0., -0.5, -0.5, -0.5, -1., -1.5],
        ),
        (
            "warm",
            "温暖轻柔",
            "柔和高频并增加低频厚度",
            [1., 1.5, 1., 0.5, 0., 0., -0.5, -1., -2., -2.5],
        ),
        (
            "retro",
            "华丽复古",
            "强调中低频并减弱高频",
            [0., 1., 2.5, 3., 2., 0.5, -0.5, -2., -3.5, -5.],
        ),
    ];
    let mut entries: Vec<_> = curves
        .into_iter()
        .map(|(id, name, description, gains_db)| Preset {
            id: PresetId::builtin(id),
            name: name.to_owned(),
            description: description.to_owned(),
            payload: Payload::Eq(EqCurve {
                gains_db,
                preamp_mode: PreampMode::Auto,
                requested_preamp_db: 0.0,
            }),
            source: PresetSource::Builtin,
        })
        .collect();
    entries.push(Preset {
        id: PresetId::builtin("surround"),
        name: "空间感增强".to_owned(),
        description: "拓宽双声道空间感；需要双声道输入".to_owned(),
        payload: Payload::Spatial(Spatial::default()),
        source: PresetSource::Builtin,
    });
    entries
}

#[must_use]
pub fn builtin(id: &PresetId) -> Option<Preset> {
    builtin_presets().into_iter().find(|entry| &entry.id == id)
}
