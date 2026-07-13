# switch2-procon

Use a Switch 2 Pro Controller on Windows PC games as an Xbox controller.


## Setup

1. Install [ViGEmBus](https://github.com/nefarius/ViGEmBus/releases) (required virtual Xbox pad driver).
2. Put the controller in pairing mode (hold the back button).
3. Run the app:

```bash
cargo run -p switch2-procon-app
```

Close the window to hide to the tray. Use the tray menu to show the window again, disconnect, or quit.

## Button / axis mapping

Position-based (Nintendo labels → Xbox labels). A/B and X/Y follow **physical position**, not printed letters.

| Switch 2 Pro | Xbox (XInput) | Notes |
|---|---|---|
| B (bottom) | A | Position mapping |
| A (right) | B | Position mapping |
| Y (left) | X | Position mapping |
| X (top) | Y | Position mapping |
| L | LB | |
| R | RB | |
| ZL | LT (255) | Digital → full press |
| ZR | RT (255) | Digital → full press |
| − (Minus) | Back | |
| + (Plus) | Start | |
| Home | Guide | Xbox Guide button |
| LS (stick click) | L Thumb | |
| RS (stick click) | R Thumb | |
| D-Pad | D-Pad | |
| Left stick | Left Stick | |
| Right stick | Right Stick | |
| Capture | — | Unmapped |
| GL / GR (rear) | — | Unmapped |
| Left rumble | Large motor | Amplitude only |
| Right rumble | Small motor | Amplitude only |

Rumble from games is forwarded to the controller. Xbox only provides motor strength (not frequency), so intensity is mapped to amplitude with a fixed vibration frequency.
