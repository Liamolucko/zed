use gpui::{Div, Render};
use story::Story;

use crate::prelude::*;
use crate::ListItem;

pub struct ListItemStory;

impl Render for ListItemStory {
    type Element = Div;

    fn render(&mut self, cx: &mut ViewContext<Self>) -> Self::Element {
        Story::container()
            .child(Story::title_for::<ListItem>())
            // .child(Story::label("Default"))
            // .child(ListItem::new("hello_world").child("Hello, world!"))
            .child(Story::label("With `on_click`"))
            .child(
                ListItem::new("with_on_click")
                    .child("Click me")
                    .on_click(|_event, _cx| {
                        println!("Clicked!");
                    }),
            )
    }
}
