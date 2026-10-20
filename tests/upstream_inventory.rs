//! Fixed snapshot scope: directory presence alone is not source registration.
use pansou::{
    config::{Config, UPSTREAM_DEFAULT_PROVIDERS},
    providers::builtin_stateless_providers,
};
use std::collections::BTreeSet;

#[test]
fn october_snapshot_registers_exactly_106_unique_sources() {
    let expected: BTreeSet<_> = "pansearch yunsou djgou hdmoli meitizy yulinshufa clxiong jsnoteclub duanjuw dyyj jupansou cldi clmao cyg susu qqpd weibo gying panlian 488run 5266ys ahhhhfs aikanzy aipan alupan ash bixin btbtlb buerchen daishudj diduan discourse duoduo dy4k dyyjpro erxiao erxiaopan feikuai gaoqing888 haisou hdr4k hjzhencai huban hunhepan ikantv javdb jikepan jutoushe kkmao kkv kpkuang labi leijing leso libvio lingjisp lou1 melost miaoso mikuclub miosou mizixing muou nsgame nyaa ouge pan365 pan666 panta panwiki panyq panzun pianku qingying qiwei quark4k quarkres quarksoo quarktv qupanshe qupansou sdso shandian sopanya sousou thepiratebay u3c3 wanou woniu wuji xb6v xdpan xdyh xiaoji xiaoyu xiaozhang xinjuc xys yingso yiove ypfxw yuhuage yunso zhizhen zlxapp zxzj".split_whitespace().collect();
    assert_eq!(expected.len(), 106);
    let providers = builtin_stateless_providers();
    let mut actual: BTreeSet<_> = providers.iter().map(|p| p.meta().name).collect();
    assert_eq!(
        actual.len(),
        providers.len(),
        "duplicate stateless registration"
    );
    for name in ["qqpd", "weibo", "gying", "panlian", "woniu"] {
        assert!(
            actual.insert(name),
            "stateful source registered as stateless: {name}"
        );
    }
    assert_eq!(actual, expected);
}

#[test]
fn default_sources_are_the_fixed_upstream_intersection() {
    let providers = builtin_stateless_providers();
    let registered: BTreeSet<_> = providers
        .iter()
        .map(|p| p.meta().name)
        .chain(["qqpd", "weibo", "gying", "panlian", "woniu"])
        .collect();
    let expected: Vec<String> = UPSTREAM_DEFAULT_PROVIDERS
        .iter()
        .filter(|name| registered.contains(**name))
        .map(|name| (*name).to_owned())
        .collect();
    assert_eq!(expected.len(), 66);
    assert_eq!(Config::default().search.providers, expected);
}
