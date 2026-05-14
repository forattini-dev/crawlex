// Selector engine over the v2 parser tree.
//
// Three query surfaces:
//   * `css(sel)`              → `Vec<ElementHandle>` (Scrapy-compatible
//                                CSS minus pseudo-elements; pseudos are
//                                stripped silently so a selector like
//                                `a::attr(href)` still picks `<a>` nodes).
//   * `css_get_all(sel)`      → `Vec<String>` honoring Scrapy/Parsel
//                                pseudos `::text` and `::attr(name)`.
//   * `xpath(expr)`           → `Vec<ElementHandle>` for a hand-rolled
//                                XPath subset: location paths with axes
//                                (`child`, `descendant`, `descendant-or-self`,
//                                `parent`, `ancestor`, `ancestor-or-self`,
//                                `self`, `following-sibling`,
//                                `preceding-sibling`), name tests, `*`,
//                                and predicates `[@attr]`, `[@attr='v']`,
//                                `[N]`. Terminal `text()` / `@attr` are
//                                handled by `xpath_get_all`.
//
// Element navigation methods (`parent`, `siblings`, `children`) walk the
// underlying `ego_tree` and skip non-element nodes.

use ego_tree::NodeRef;
use scraper::{node::Node, ElementRef, Selector};

use super::TreeHandle;

#[derive(Clone, Copy)]
pub struct ElementHandle<'a> {
    node: ElementRef<'a>,
}

impl<'a> ElementHandle<'a> {
    pub fn from(node: ElementRef<'a>) -> Self {
        Self { node }
    }

    pub fn inner(&self) -> ElementRef<'a> {
        self.node
    }

    pub fn name(&self) -> &str {
        self.node.value().name()
    }

    pub fn attr(&self, name: &str) -> Option<&'a str> {
        self.node.value().attr(name)
    }

    pub fn text(&self) -> String {
        self.node.text().collect::<Vec<_>>().concat()
    }

    pub fn html(&self) -> String {
        self.node.html()
    }

    pub fn inner_html(&self) -> String {
        self.node.inner_html()
    }

    pub fn children(&self) -> Vec<ElementHandle<'a>> {
        self.node
            .children()
            .filter_map(ElementRef::wrap)
            .map(ElementHandle::from)
            .collect()
    }

    pub fn parent(&self) -> Option<ElementHandle<'a>> {
        let mut cur = self.node.parent();
        while let Some(n) = cur {
            if let Some(el) = ElementRef::wrap(n) {
                return Some(ElementHandle::from(el));
            }
            cur = n.parent();
        }
        None
    }

    pub fn siblings(&self) -> Vec<ElementHandle<'a>> {
        let parent = match self.node.parent() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let self_id = self.node.id();
        parent
            .children()
            .filter_map(ElementRef::wrap)
            .filter(|n| n.id() != self_id)
            .map(ElementHandle::from)
            .collect()
    }

    pub fn css(&self, sel: &str) -> Vec<ElementHandle<'a>> {
        css_select(self.node, sel)
    }

    pub fn xpath(&self, expr: &str) -> Vec<ElementHandle<'a>> {
        xpath_select(self.node, expr)
    }
}

// ---------- CSS ----------

#[derive(Debug, Clone, PartialEq)]
enum Pseudo {
    None,
    Text,
    Attr(String),
}

fn strip_pseudo(sel: &str) -> (String, Pseudo) {
    if let Some(idx) = sel.rfind("::") {
        let (base, tail) = sel.split_at(idx);
        let after = &tail[2..];
        if after == "text" {
            return (base.trim().to_string(), Pseudo::Text);
        }
        if let Some(rest) = after.strip_prefix("attr(") {
            if let Some(name) = rest.strip_suffix(')') {
                return (base.trim().to_string(), Pseudo::Attr(name.to_string()));
            }
        }
    }
    (sel.to_string(), Pseudo::None)
}

fn css_select<'a>(root: ElementRef<'a>, sel: &str) -> Vec<ElementHandle<'a>> {
    let (base, _) = strip_pseudo(sel);
    let parsed = match Selector::parse(&base) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    root.select(&parsed).map(ElementHandle::from).collect()
}

// ---------- XPath ----------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Axis {
    SelfNode,
    Child,
    Descendant,
    DescendantOrSelf,
    Parent,
    Ancestor,
    AncestorOrSelf,
    FollowingSibling,
    PrecedingSibling,
}

#[derive(Debug, Clone, PartialEq)]
enum NameTest {
    Star,
    Name(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Predicate {
    HasAttr(String),
    AttrEq(String, String),
    Position(usize),
}

#[derive(Debug, Clone, PartialEq)]
enum Terminal {
    Element,
    AttrValue(String),
    Text,
}

#[derive(Debug, Clone)]
struct Step {
    axis: Axis,
    name: NameTest,
    predicates: Vec<Predicate>,
}

#[derive(Debug, Clone)]
struct Path {
    absolute: bool,
    steps: Vec<Step>,
    terminal: Terminal,
}

fn parse_xpath(expr: &str) -> Option<Path> {
    let mut s = expr.trim();
    let absolute = s.starts_with('/');
    let mut steps = Vec::new();
    let mut terminal = Terminal::Element;

    // Leading `//` becomes a synthetic descendant-or-self step. After
    // that, every `/` is just a step separator and every `//` injects
    // another descendant-or-self::node().
    if let Some(rest) = s.strip_prefix("//") {
        steps.push(Step {
            axis: Axis::DescendantOrSelf,
            name: NameTest::Star,
            predicates: Vec::new(),
        });
        s = rest;
    } else if let Some(rest) = s.strip_prefix('/') {
        s = rest;
    }

    loop {
        s = s.trim_start();
        if s.is_empty() {
            break;
        }

        // Terminal forms — must be the last token.
        if let Some(rest) = s.strip_prefix('@') {
            let (name, tail) = take_name(rest);
            if name.is_empty() {
                return None;
            }
            terminal = Terminal::AttrValue(name);
            s = tail;
            break;
        }
        if let Some(rest) = s.strip_prefix("text()") {
            terminal = Terminal::Text;
            s = rest;
            break;
        }

        let (step, rest) = parse_step(s)?;
        steps.push(step);
        s = rest.trim_start();

        if let Some(r) = s.strip_prefix("//") {
            steps.push(Step {
                axis: Axis::DescendantOrSelf,
                name: NameTest::Star,
                predicates: Vec::new(),
            });
            s = r;
        } else if let Some(r) = s.strip_prefix('/') {
            s = r;
        } else {
            break;
        }
    }

    if !s.trim().is_empty() {
        return None;
    }

    Some(Path { absolute, steps, terminal })
}

fn parse_step(input: &str) -> Option<(Step, &str)> {
    let (axis, rest) = parse_axis(input);
    let s = rest.trim_start();

    let (name, mut s) = if let Some(r) = s.strip_prefix('*') {
        (NameTest::Star, r)
    } else {
        let (n, r) = take_name(s);
        if n.is_empty() {
            return None;
        }
        (NameTest::Name(n), r)
    };

    let mut predicates = Vec::new();
    loop {
        s = s.trim_start();
        if let Some(r) = s.strip_prefix('[') {
            let (pred, after) = parse_predicate(r)?;
            predicates.push(pred);
            s = after;
        } else {
            break;
        }
    }

    Some((Step { axis, name, predicates }, s))
}

fn parse_axis(input: &str) -> (Axis, &str) {
    let axes: &[(&str, Axis)] = &[
        ("descendant-or-self::", Axis::DescendantOrSelf),
        ("ancestor-or-self::", Axis::AncestorOrSelf),
        ("following-sibling::", Axis::FollowingSibling),
        ("preceding-sibling::", Axis::PrecedingSibling),
        ("descendant::", Axis::Descendant),
        ("ancestor::", Axis::Ancestor),
        ("parent::", Axis::Parent),
        ("child::", Axis::Child),
        ("self::", Axis::SelfNode),
    ];
    for (prefix, axis) in axes {
        if let Some(rest) = input.strip_prefix(prefix) {
            return (*axis, rest);
        }
    }
    (Axis::Child, input)
}

fn take_name(input: &str) -> (String, &str) {
    let end = input
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':'))
        .unwrap_or(input.len());
    (input[..end].to_string(), &input[end..])
}

fn parse_predicate(input: &str) -> Option<(Predicate, &str)> {
    let close = input.find(']')?;
    let body = input[..close].trim();
    let after = &input[close + 1..];

    if let Ok(n) = body.parse::<usize>() {
        if n >= 1 {
            return Some((Predicate::Position(n), after));
        }
    }
    if let Some(attr_body) = body.strip_prefix('@') {
        if let Some(eq) = attr_body.find('=') {
            let name = attr_body[..eq].trim().to_string();
            let rhs = attr_body[eq + 1..].trim();
            let val = rhs
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
                .or_else(|| rhs.strip_prefix('"').and_then(|s| s.strip_suffix('"')))?;
            return Some((Predicate::AttrEq(name, val.to_string()), after));
        }
        return Some((Predicate::HasAttr(attr_body.trim().to_string()), after));
    }
    None
}

fn xpath_select<'a>(root: ElementRef<'a>, expr: &str) -> Vec<ElementHandle<'a>> {
    let path = match parse_xpath(expr) {
        Some(p) => p,
        None => return Vec::new(),
    };
    if !matches!(path.terminal, Terminal::Element) {
        return Vec::new();
    }
    let start_nodes: Vec<NodeRef<'a, Node>> = if path.absolute {
        // From document root — walk up to the topmost node.
        let mut top = *root;
        while let Some(p) = top.parent() {
            top = p;
        }
        vec![top]
    } else {
        vec![*root]
    };
    let mut current: Vec<NodeRef<'a, Node>> = start_nodes;
    for step in &path.steps {
        let mut next: Vec<NodeRef<'a, Node>> = Vec::new();
        for ctx in &current {
            let candidates = apply_axis(*ctx, step.axis);
            let matched: Vec<NodeRef<'a, Node>> = candidates
                .into_iter()
                .filter(|n| name_matches(*n, &step.name))
                .collect();
            // Predicates evaluate against the matched node list per ctx.
            let after_pred = apply_predicates(matched, &step.predicates);
            next.extend(after_pred);
        }
        next = dedupe_nodes(next);
        current = next;
    }
    current
        .into_iter()
        .filter_map(ElementRef::wrap)
        .map(ElementHandle::from)
        .collect()
}

fn apply_axis<'a>(ctx: NodeRef<'a, Node>, axis: Axis) -> Vec<NodeRef<'a, Node>> {
    match axis {
        Axis::SelfNode => vec![ctx],
        Axis::Child => ctx.children().collect(),
        Axis::Descendant => descendants(ctx, false),
        Axis::DescendantOrSelf => descendants(ctx, true),
        Axis::Parent => ctx.parent().into_iter().collect(),
        Axis::Ancestor => ancestors(ctx, false),
        Axis::AncestorOrSelf => ancestors(ctx, true),
        Axis::FollowingSibling => {
            let mut out = Vec::new();
            let mut s = ctx.next_sibling();
            while let Some(n) = s {
                out.push(n);
                s = n.next_sibling();
            }
            out
        }
        Axis::PrecedingSibling => {
            let mut out = Vec::new();
            let mut s = ctx.prev_sibling();
            while let Some(n) = s {
                out.push(n);
                s = n.prev_sibling();
            }
            out
        }
    }
}

fn descendants<'a>(root: NodeRef<'a, Node>, include_self: bool) -> Vec<NodeRef<'a, Node>> {
    let mut out = Vec::new();
    if include_self {
        out.push(root);
    }
    let mut stack: Vec<NodeRef<'a, Node>> = root.children().collect::<Vec<_>>();
    stack.reverse();
    while let Some(n) = stack.pop() {
        out.push(n);
        let kids: Vec<_> = n.children().collect();
        for c in kids.into_iter().rev() {
            stack.push(c);
        }
    }
    out
}

fn ancestors<'a>(node: NodeRef<'a, Node>, include_self: bool) -> Vec<NodeRef<'a, Node>> {
    let mut out = Vec::new();
    if include_self {
        out.push(node);
    }
    let mut cur = node.parent();
    while let Some(n) = cur {
        out.push(n);
        cur = n.parent();
    }
    out
}

fn name_matches(node: NodeRef<'_, Node>, test: &NameTest) -> bool {
    let el = match node.value().as_element() {
        Some(e) => e,
        None => return false,
    };
    match test {
        NameTest::Star => true,
        NameTest::Name(n) => el.name() == n.as_str(),
    }
}

fn apply_predicates<'a>(
    nodes: Vec<NodeRef<'a, Node>>,
    preds: &[Predicate],
) -> Vec<NodeRef<'a, Node>> {
    let mut current = nodes;
    for p in preds {
        match p {
            Predicate::Position(n) => {
                let idx = n - 1;
                current = current.into_iter().nth(idx).into_iter().collect();
            }
            Predicate::HasAttr(name) => {
                current = current
                    .into_iter()
                    .filter(|node| {
                        node.value()
                            .as_element()
                            .and_then(|e| e.attr(name))
                            .is_some()
                    })
                    .collect();
            }
            Predicate::AttrEq(name, val) => {
                current = current
                    .into_iter()
                    .filter(|node| {
                        node.value()
                            .as_element()
                            .and_then(|e| e.attr(name))
                            .map(|v| v == val.as_str())
                            .unwrap_or(false)
                    })
                    .collect();
            }
        }
    }
    current
}

fn dedupe_nodes<'a>(nodes: Vec<NodeRef<'a, Node>>) -> Vec<NodeRef<'a, Node>> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(nodes.len());
    for n in nodes {
        if seen.insert(n.id()) {
            out.push(n);
        }
    }
    out
}

// ---------- TreeHandle integration ----------

impl TreeHandle {
    pub fn css(&self, sel: &str) -> Vec<ElementHandle<'_>> {
        css_select(self.root_element(), sel)
    }

    /// Scrapy/Parsel-compatible extraction. `sel` may include a trailing
    /// `::text` or `::attr(name)` pseudo to pull strings instead of
    /// elements.
    pub fn css_get_all(&self, sel: &str) -> Vec<String> {
        let (base, pseudo) = strip_pseudo(sel);
        let parsed = match Selector::parse(&base) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        self.html()
            .select(&parsed)
            .filter_map(|n| match &pseudo {
                Pseudo::None => Some(n.html()),
                Pseudo::Text => Some(n.text().collect::<Vec<_>>().concat()),
                Pseudo::Attr(name) => n.value().attr(name).map(|s| s.to_string()),
            })
            .collect()
    }

    pub fn css_get(&self, sel: &str) -> Option<String> {
        self.css_get_all(sel).into_iter().next()
    }

    pub fn xpath(&self, expr: &str) -> Vec<ElementHandle<'_>> {
        xpath_select(self.root_element(), expr)
    }

    /// XPath variant that resolves `text()` / `@attr` terminals to
    /// strings. Element-terminal expressions return serialised outer HTML.
    pub fn xpath_get_all(&self, expr: &str) -> Vec<String> {
        let path = match parse_xpath(expr) {
            Some(p) => p,
            None => return Vec::new(),
        };
        let root = self.root_element();
        // Re-walk steps using xpath_select for element terminal cases.
        match &path.terminal {
            Terminal::Element => xpath_select(root, expr)
                .into_iter()
                .map(|h| h.html())
                .collect(),
            Terminal::Text => {
                // Strip terminal, walk to elements, then collect their text.
                let stripped = expr.trim_end_matches("text()").trim_end_matches('/');
                xpath_select(root, stripped)
                    .into_iter()
                    .map(|h| h.text())
                    .collect()
            }
            Terminal::AttrValue(name) => {
                // Strip trailing /@attr.
                let cut = expr.rfind('@').unwrap();
                let stripped = expr[..cut].trim_end_matches('/');
                xpath_select(root, stripped)
                    .into_iter()
                    .filter_map(|h| h.attr(name).map(|s| s.to_string()))
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::parse_tree;

    const PAGE: &[u8] = br#"<!doctype html>
<html><body>
  <section id="main">
    <h1>Title</h1>
    <p class="lead">first <em>paragraph</em></p>
    <ul>
      <li class="item" data-id="1">alpha</li>
      <li class="item" data-id="2">beta</li>
      <li class="item" data-id="3">gamma</li>
    </ul>
    <a href="https://example.com/a">A</a>
    <a href="https://example.com/b">B</a>
  </section>
</body></html>"#;

    #[test]
    fn css_basic() {
        let t = parse_tree(PAGE, None);
        let lis = t.css("li.item");
        assert_eq!(lis.len(), 3);
        assert_eq!(lis[0].attr("data-id"), Some("1"));
        assert_eq!(lis[0].text(), "alpha");
    }

    #[test]
    fn css_pseudo_text_scrapy_parity() {
        let t = parse_tree(PAGE, None);
        let txt = t.css_get_all("li.item::text");
        assert_eq!(txt, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn css_pseudo_attr_scrapy_parity() {
        let t = parse_tree(PAGE, None);
        let hrefs = t.css_get_all("a::attr(href)");
        assert_eq!(
            hrefs,
            vec!["https://example.com/a", "https://example.com/b"]
        );
        assert_eq!(
            t.css_get("a::attr(href)").as_deref(),
            Some("https://example.com/a"),
        );
    }

    #[test]
    fn navigation_parent_children_siblings() {
        let t = parse_tree(PAGE, None);
        let first = t.css("li.item").into_iter().next().unwrap();
        let parent = first.parent().unwrap();
        assert_eq!(parent.name(), "ul");
        let children = parent.children();
        assert_eq!(children.len(), 3);
        let sibs = first.siblings();
        assert_eq!(sibs.len(), 2);
        assert_eq!(sibs[0].attr("data-id"), Some("2"));
    }

    #[test]
    fn xpath_descendants_and_predicates() {
        let t = parse_tree(PAGE, None);
        let all = t.xpath("//li");
        assert_eq!(all.len(), 3);
        let pick = t.xpath("//li[@data-id='2']");
        assert_eq!(pick.len(), 1);
        assert_eq!(pick[0].text(), "beta");
        let by_pos = t.xpath("//ul/li[3]");
        assert_eq!(by_pos.len(), 1);
        assert_eq!(by_pos[0].text(), "gamma");
    }

    #[test]
    fn xpath_axis_ancestor() {
        let t = parse_tree(PAGE, None);
        let em = t.css("p.lead em").into_iter().next().unwrap();
        let secs = em.xpath("ancestor::section");
        assert_eq!(secs.len(), 1);
        assert_eq!(secs[0].attr("id"), Some("main"));
    }

    #[test]
    fn xpath_axis_following_sibling() {
        let t = parse_tree(PAGE, None);
        let first_li = t.css("li.item").into_iter().next().unwrap();
        let rest = first_li.xpath("following-sibling::li");
        assert_eq!(rest.len(), 2);
        assert_eq!(rest[0].text(), "beta");
        assert_eq!(rest[1].text(), "gamma");
    }

    #[test]
    fn xpath_attr_terminal() {
        let t = parse_tree(PAGE, None);
        let hrefs = t.xpath_get_all("//a/@href");
        assert_eq!(
            hrefs,
            vec!["https://example.com/a", "https://example.com/b"]
        );
    }

    #[test]
    fn xpath_text_terminal() {
        let t = parse_tree(PAGE, None);
        let texts = t.xpath_get_all("//li/text()");
        assert_eq!(texts, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn xpath_star_name_test() {
        let t = parse_tree(PAGE, None);
        let kids = t.xpath("//ul/*");
        assert_eq!(kids.len(), 3);
        for k in kids {
            assert_eq!(k.name(), "li");
        }
    }

    #[test]
    fn css_invalid_returns_empty() {
        let t = parse_tree(PAGE, None);
        let v = t.css("[[invalid");
        assert!(v.is_empty());
    }
}
