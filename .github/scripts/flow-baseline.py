"""Restore only pre-fix behavior, keeping all newly compiled UI regressions."""
from pathlib import Path
p = Path("src/ui/impls/sidebar.rs")
s = p.read_text()
s = s.replace("        // An explicit resize supersedes an in-flight expansion, including a\n        // same-width grab. Its old target must not overwrite the new width.\n        self.anim = None;\n", "", 1)
s = s.replace("        if (self.shown_width - clamped).abs() < 0.5\n            && (self.width - clamped).abs() < 0.5\n        {", "        if (self.shown_width - clamped).abs() < 0.5 {", 1)
p.write_text(s)
p = Path("src/ui/impls/pages/terminal_page.rs")
s = p.read_text()
s = s.replace("                        && !this.sidebar.read(cx).is_collapsed()\n", "", 1)
start = s.index("                if event.pressed_button != Some(MouseButton::Left) {", s.index("            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent"))
end = s.index("                if let Some((start_x, start_width)) = this.sidebar_drag {", start)
s = s[:start] + s[end:]
p.write_text(s)
