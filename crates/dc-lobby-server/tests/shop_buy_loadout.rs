//! lobby-3 (user: "if we buy certain weapons we should see them appear in game too"): a GP weapon bought in the shop
//! (0x328), equipped (0x324) and given a bought option (0x336) is what the lobby hands the game server for the next
//! match: the player's `user` line carries the weapon in its slot (`loadout=`) and the option (`parts=`). Real lobby
//! binary, stand-in game server that records its control lines, the user's ROM shop list (skipped without CA_ROM).
// SPEC: PROTOCOL_2009.md §4.2 (0x328 / 0x329, 0x324 / 0x325, 0x336 / 0x337); DSERVER_PIPE.md §3.1 (user line);
// DSERVER_PIPE_addendum_parts.md §1 (`parts=`)
// HARNESS: loadout_tokens.rs (same process-test style)

mod support;
use std::time::Duration;
use support::*;

const T: Duration = Duration::from_secs(20);

/// A shown GP product of exactly one item (consume 0) whose item satisfies `want`, cheapest first: (product, item).
fn gp_product(
    cat: &dc_bute::ShopCatalog,
    items: &dc_bute::ItemsTable,
    want: impl Fn(&dc_bute::ItemsTable, i32) -> bool,
) -> Option<(i32, i32)> {
    cat.view_list()
        .into_iter()
        .filter_map(|pid| {
            let p = cat.product(pid)?;
            if p.sale_type != 0 || p.items.len() != 1 || p.items[0].1 != 0 || p.sale_price <= 0 {
                return None;
            }
            let item = cat.info(p.items[0].0)?.itemid;
            want(items, item).then_some((p.sale_price, pid, item))
        })
        .min()
        .map(|(_, pid, item)| (pid, item))
}

fn tok<'a>(line: &'a [String], key: &str) -> Option<&'a str> {
    line.iter()
        .skip(7)
        .find_map(|t| t.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

/// 0x328 for `product` (option 0, nothing to extend); the new unique id from 0x329.
fn buy(c: &mut Client, product: i32) -> u32 {
    c.send(0x328, |w| {
        w.i32(product).u32(0);
        for _ in 0..5 {
            w.u32(0);
        }
    });
    let r = c.recv_until(0x329, T);
    assert_eq!(
        u32_at(&r, 6),
        1,
        "purchase of {product} refused: reason 0x{:x}",
        u16_at(&r, 0xa)
    );
    u32_at(&r, 0xe)
}

#[test]
fn a_bought_and_equipped_weapon_with_its_bought_option_reaches_the_game_server() {
    let Some(rom) = dc_rom::test_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let (Ok(cat), Ok(items)) = (
        dc_bute::ShopCatalog::from_rom(&rom),
        dc_bute::ItemsTable::from_rom(&rom),
    ) else {
        eprintln!("skipped: no shop list / item table in the ROM");
        return;
    };
    // a primary weapon (equipment slot 0) with an open sight socket and a sight option (kind 0x12); CLI-1606: the
    // lobby refuses an option into a closed socket (the cheapest primary, the Remington 870, has SightOption 0)
    let sight_open: std::collections::HashMap<i32, bool> = dc_bute::WeaponsTable::from_rom(&rom)
        .map(|t| {
            t.option_socket_states()
                .into_iter()
                .map(|(n, s)| (n, s[1] == 1))
                .collect()
        })
        .unwrap_or_default();
    let slot_of = |t: &dc_bute::ItemsTable, n: i32| {
        (0..1000)
            .contains(&n)
            .then(|| dc_bute::equip_slot_by_class_code(t.weapon_class_code(n)))
            .flatten()
    };
    let Some((weapon_pid, weapon)) = gp_product(&cat, &items, |t, n| {
        slot_of(t, n) == Some(0) && sight_open.get(&n).copied().unwrap_or(true)
    }) else {
        eprintln!("skipped: no GP primary weapon product");
        return;
    };
    let option = gp_product(&cat, &items, |t, n| {
        (1000..2000).contains(&n) && t.option_kind(n) == 0x12
    });
    let l = Lobby::start(
        "shop-buy-loadout",
        LobbyOpts {
            dserver_exe: Some(fake_dserver_exe()),
            dserver_world: Some(r"Worlds\SnowValley".into()),
            extra: vec!["--start-gp".into(), "10000000".into()],
            ..Default::default()
        },
    );
    l.set_fake_mode("record");
    let (mut a, na) = player(&l, "alice", "Alice");
    let (mut b, _nb) = player(&l, "bob", "Bob");
    let before: Vec<i64> = {
        let db = l.db();
        let mut q = db
            .prepare("SELECT item_no FROM items WHERE user_no=?1 AND equip_slot=0")
            .unwrap();
        let v = q
            .query_map([na as i64], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        v
    };
    assert!(
        !before.contains(&(weapon as i64)),
        "the test weapon {weapon} is not the starter primary"
    );
    // buy and equip the weapon in slot 0
    let uid = buy(&mut a, weapon_pid);
    assert_ne!(uid, 0);
    a.send(0x324, |w| {
        w.u32(uid).u32(weapon as u32).u32(0);
    });
    assert_eq!(u32_at(&a.recv_until(0x325, T), 6), 1, "equip refused");
    a.send(0x322, |_| {});
    let eq = a.recv_until(0x323, T);
    assert_eq!(
        u32_at(&eq, 0x12) as i32,
        weapon,
        "0x323 slot 0 holds the bought weapon"
    );
    // buy an option and attach it to the weapon
    let opt = option.map(|(pid, n)| {
        let ouid = buy(&mut a, pid);
        a.send(0x336, |w| {
            w.u32(ouid).u32(n as u32).u32(uid);
        });
        assert_eq!(u32_at(&a.recv_until(0x337, T), 6), 1, "attach refused");
        n
    });
    // the next match
    let no = open_room(&mut a, "Bought kit", 0);
    join_room(&mut b, no);
    ready(&mut b, 1);
    ready(&mut a, 2);
    let _ = game_address(&mut a, T);
    let _ = game_address(&mut b, T);
    assert!(
        wait_until(T, || l.fake().first().is_some_and(|f| f.users().len() == 2)),
        "two user lines expected:\n{}",
        l.log_text()
    );
    let users = l.fake()[0].users();
    let line = users
        .iter()
        .find(|u| u[1] == na.to_string())
        .unwrap_or_else(|| panic!("no user line for {na}: {users:?}"));
    let loadout = tok(line, "loadout").unwrap_or_default();
    assert!(
        loadout.split(',').any(|e| e == format!("0:{weapon}")),
        "slot 0 carries the bought weapon {weapon}: {line:?}"
    );
    if let Some(n) = opt {
        let parts = tok(line, "parts").unwrap_or_default();
        assert!(
            parts
                .split(',')
                .any(|e| e.starts_with("0:") && e.split('/').any(|x| x == n.to_string())),
            "slot 0's parts carry the bought option {n}: {line:?}"
        );
    } else {
        eprintln!("note: no GP sight option product in this ROM; parts not checked");
    }
}
