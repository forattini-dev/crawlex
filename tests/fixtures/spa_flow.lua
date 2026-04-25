-- Live SPA flow driven from Lua: click button, wait for the post-pushState
-- selector, then take an element screenshot. Asserted from the Rust side.
--
-- The hook fires during the render's AfterLoad stage — the page is already
-- navigated and idle, so helpers see the initial DOM.
function on_after_load(ctx)
  -- Make sure the button is actually present before we click. `page_wait_for`
  -- is the Playwright-style alias; same as `page_wait`.
  page_wait_for("#go", 3000)
  page_click("#go")
  -- pushState flips the view in; give it up to 5s.
  page_wait_for("#dashboard", 5000)
  return "continue"
end
