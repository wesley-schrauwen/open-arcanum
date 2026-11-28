# Checks by eye

Each item below is built and passes its tests, but nobody has looked at it in the game yet.
Tick an item once it has been seen working. Note anything wrong under it, with the PR it came
from. Each new PR adds its own items here. How to run the game for a check: `private/DEVELOPMENT.md` §6.

## Barter (#79, #88)

- [ ] Talking to a shopkeeper and choosing to trade opens the barter window: the PC's items on
      one side, the shop's on the other, and the shopkeeper's name shown.
- [ ] Hovering an item shows its price on the barter line. Buying takes the gold and moves the
      item over; selling does the reverse.
- [ ] Items the shop won't take, and buying without enough gold, show the refusal and change
      nothing.
- [ ] Closing and reopening the window shows the same stock. After 12-24 game hours (rest), the
      stock has been restocked.
- [ ] Leaving the map with the window open closes it.

## Vote popup (#81, #89)

Needs two players: a LAN host and a client (`DEBUG_ARCANUM_LISTEN` / `_JOIN`).

- [ ] When one player triggers a teleport, the other sees the black panel with the title
      "Confirm Group Action", then "<name> wants to go elsewhere. Go along?", then Yes and No.
- [ ] The text fits the panel; Yes and No turn cyan under the cursor.
- [ ] Both answer Yes: the party moves. Either answers No: nobody moves and the panel goes.
- [ ] The game clock keeps running and both players can act while the vote is open.
- [ ] A player who leaves during a vote drops out of it, and the vote still ends.
- [ ] Alone in the world, a teleport happens at once, with no panel.

## Teleports and map switches (#81, #82, #83)

- [ ] A script teleport within the map moves the party, and the camera follows.
- [ ] A teleport to another map (for example into Bessie Toone's mine, map 5001) clears the old
      map's sprites and shows the new map with its NPCs.
- [ ] Coming back shows the first map as it was left: changes are kept (moved and taken items,
      NPC inventories).
- [ ] Stepping on a jump point (`map.jmp`) starts the move, voted on as above.

## Dialogue (#78, #80, #86, #87)

- [ ] NPCs without a script greet by class or race and reaction, and bark when passed.
- [ ] Rumours, training and quest lines show their prices and take the gold.
- [ ] Directions (`D:` / `X:`) give a sensible direction and distance and mark the map.

## Effects (script effects PR)

- [ ] Putting on a magic item (one with a wield script) changes the character screen (a stat,
      a resistance or the pools); taking it off changes it back.
- [ ] Other players' party bars show the new maximum hit points or fatigue.

## Switching objects off and on (script gaps 4 PR)

- [ ] When a script switches an object off, it disappears for every player and stops blocking;
      switched on, it comes back in its place. An NPC switched off despawns.
- [ ] The redrawn sector shows no gap or flicker beyond the one frame.

## Levelling (levelling PR)

- [ ] A dialogue line that gives experience raises the level when enough is reached, shows
      "Congratulations! You are now level N." in the message box, and the points appear.
- [ ] The character screen shows the new level and the experience still needed ("XP Next").

## Followers (followers PR)

- [ ] Virgil joins after "I suppose it can not hurt to search out this Elder Joachim" and walks
      after the PC, stopping about 4 tiles away; after `so` (spread out) about 7.
- [ ] Told to wait, he stays; told to rejoin, he follows again; told to leave, he stops following.
- [ ] Far away (31 tiles or more), he appears beside the PC.
- [ ] A teleport within the map brings him along; a map change leaves him waiting behind.
- [ ] Refusals show the NPC's line and end the talk.

## Healing (dialogue healing PR)

- [ ] An NPC with an `H:` option shows "I need healing."-type text; choosing it shows only what
      that NPC can do, then the price, then "It is done."-type line and a thanks.
- [ ] The gold goes; Virgil (a follower) heals for nothing.
- [ ] The party bar's hit points rise after healing once something can hurt the PC (nothing does
      yet).

## Party travel (party travel PR)

- [ ] With two players, a dialogue line that gives experience raises both; each sees their own
      level-up line.
- [ ] Virgil, following, comes along through a map change and arrives beside the PC; told to
      wait first, he stays behind; going back, he is home again.

## Script gaps 5

- [ ] A plaque's text appears in the message box when a script shows it.
- [ ] An object a script transforms or gives new art is redrawn for every player; one destroyed
      disappears.

## Heartbeats and killing

- [ ] At the crash site the bodies lie dead (death animation, last frame) instead of standing.
- [ ] NPCs with heartbeat scripts do their thing (switching on or off, barks) while the PC is
      near; nothing stutters.

## Follower details

- [ ] Virgil told to wait leaves for good after two game hours (rest to check); rejoined before
      then, he stays.
- [ ] A follower whose reaction falls near its limit says a warning line; past it, it leaves with
      its refusal line.
- [ ] A `$$` price in dialogue counts followers' gold.

## Alignment and fate points

- [ ] After a dialogue result moves the alignment, the character screen's dial points to it.
- [ ] The top bar shows the fate points (two digits) beside the fate button; F or the button opens
      the fate window under the bar; "Full heal now" spends one and heals; a row clicked again
      gives the point back.

## Script gaps 6

- [ ] An NPC a script makes follow (action 0x15) walks after the PC; one told to stop (0x16)
      stays where it is.
- [ ] An NPC whose day standpoint a script moves (0x2d) walks to the new place in daytime.

## Traps

- [ ] Using a trapped chest at the start (no Spot Trap) shows "Trap!" over the PC, the party bar's
      hit points drop, and the chest stays shut; using it again opens it.
- [ ] A PC with Spot Trap sometimes stops before using it with "You have spotted a trap!".

## Skills view

- [ ] The skills button (key, right of the message box) swaps the message box for four buttons:
      mask, hand, trap jaws, tools; pressing it again, or the spells button, swaps back.
- [ ] The trap button, then a click on a door or chest with a spotted trap, walks the PC there
      and says "Trap successfully disarmed." or "Trap disarm failed.".

## Item and lock events

- [ ] Picking up, dropping and looting items behaves as before (items with get/drop scripts are
      rare; nothing should be kept back without a reason).

## Written window

- [ ] A script plaque (for example the shrine plaques, `mes/gameplaque.mes` 1000) opens the
      stone plaque with centred white text; a click closes it.
- [ ] A book opens on two pages; the arrows turn two pages; text runs on across the pages.
- [ ] Art 494 leaves a round hole at the top left that shows the world: check whether the
      original shows the same.

## Hover names

- [ ] Resting the cursor on a door, chest, item or NPC shows its name in the message box; moving
      off clears it.
- [ ] An NPC not yet talked to shows its unknown name ("Man", "Elf"…); after talking, its own.
- [ ] Magick or tech items not identified show their unknown name.

## Hover details

Checked by screenshot (`DEBUG_ARCANUM_HOVER`): the PC (level, gauges, 30/30), the half-ogre
guard (race icon, level, 100% gauges) and the locked chest (icon, "Locked", 100/100).
- [ ] Compare with the original: the half-ogre icon (`RaceHOgreIcon.ART`, 382) shows a gun.
- [ ] A weapon on the ground shows its stats row (`D:2-8  TH:+5`), weight and hit points; a
      magick one's name in light red.
- [ ] A door shows the door icon; a window the window icon.

## Reading items

- [ ] A book or note picked up opens in the written window when dropped on the inventory's use
      box, or right-clicked; the inventory closes.

## Newspapers and telegrams

- [ ] A newspaper item read from the inventory shows the Tarantian: header, headline, stories in
      columns, filler stories after.
- [ ] A telegram shows the Tarant Union Telegraph form with its message.
- [ ] Days later (rest), a script's headline float shows the day's paper.

## Newspaper sellers

- [ ] A newspaper seller's "I would like to buy a newspaper." offers today's paper, the
      headline, older papers; buying one for 2-4 coins puts a newspaper in the inventory that
      reads as that day's paper.

## Pick Pocket

- [ ] The skills view's Pick Pocket button, then a click on an NPC: the PC walks there and the
      barter window opens on the NPC's things without prices; dragging one out steals it or
      says "You fail."; caught, the window closes.

## Prowling

Checked by screenshot (`DEBUG_ARCANUM_PANEL=prowl`): the PC stands crouched (animation 5).
- [ ] Walking while prowling plays animation 5, at a believable pace; a right click stops it.
- [ ] A second player sees the first one's PC crouch.

## Combat

Checked by screenshot (`DEBUG_ARCANUM_PANEL=fight`): the AP bar over the bottom bar, two green
lights after the walk.
- [ ] The lights sit in the bar's holes; End Turn passes the turn; the NPC hits back.
- [ ] Floats: "HT 3" white over an NPC, red over the PC; "Dodge!"; "Critical hit".
- [ ] The NPC dies in its death frame; the fight ends; the bar goes.
- [ ] In combat mode, hovering an NPC shows "To Hit:" and a percentage in the icon's place.
- [ ] Monsters (wolves, kill-on-sight NPCs) attack when the PC comes near; townsfolk do not.
- [ ] Hitting a townsman: his friends of the same faction come for the PC, bystanders walk off.
- [ ] Critical floats: "Crippled arm", "Armor damaged", "Weapon broken".
- [ ] Each attack: the attacker turns to its target and swings once, then stands again.
- [ ] In combat mode the cursor is the battle cursor; over an NPC the attack cursor, its
  hotspot on the point clicked.
- [ ] A killed NPC falls through its death animation (a big hit: by the head, body or legs) and
  stays on the last frame.
- [ ] A knocked-out NPC falls and lies; when its fatigue is back it plays the get-up and stands.
- [ ] A poisoned PC or NPC floats "HT 1" (red over PCs) every 15 s of game time.
- [ ] An NPC that flees or gives up floats a gd_npc 3000 line; a follower refusing its leader's
  fight floats its reason (4800-5199).
- [ ] A blow's floats (damage, "Stunned" …) and a knock-down show as the swing lands, not as
  it starts; a thrown weapon plays the throw (14); a blow at one lying plays 21.
- [ ] In a fight, the PC and NPCs show their wielded weapon (and shield) in their art; after it,
  their hands are empty again.
- [ ] A knocked-down critter falls, lies, and gets up when the host says (up to 80 game
  seconds later).
- [ ] A PC's arrow or throw that hits a critter in the way: half the time one of its followers
  floats a gd_npc 2500 line.
- [ ] An NPC that gave up to one PC and is struck by another floats a gd_npc 3000 line at it
  and runs from it, not fighting back.
- [ ] A critical hit that knocks an NPC's weapon away floats "Weapon dropped" and the weapon lies
  on the NPC's tile; the NPC fights on bare-handed.

## Repair

- [ ] The skills view's Repair button opens the inventory; a click on a worn item shows
  "repaired" (or "not in need of repair") in the message box and leaves the item in place.

## Spotting traps while walking

- [ ] Walking towards a trap object with Spot Trap, the PC sometimes stops a tile short with
      "You have spotted a trap!".

## Lock witnesses

- [ ] A failed pick on an owned lock in sight of its owner: the owner floats "Stop that!" (or
  another gd_npc 4000 line); picked open, the owner attacks.

## Broken items

- [ ] A weapon or armour worn to nothing in a fight comes off into the pack painted red (in the
  inventory, the hotkey bar, held on the cursor and dropped on the ground); dragging it onto
  its slot shows "item.mes" 107 and leaves it in the pack.

## Examine scripts

- [ ] Hovering an NPC shows its name a moment later (the host's answer), as before otherwise.
