use ambient_api::prelude::*;

use packages::tangent_schema::concepts::PlayerClass;

#[main]
pub fn main() {
    PlayerClass {
        is_class: (),

        name: "Scout".to_string(),
        description: "Swift and elusive, ideal for hit-and-run tactics and recon missions."
            .to_string(),
        icon_url: packages::this::assets::url("icon.png"),
    }
    .spawn();
}
