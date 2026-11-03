// Generated into every guide copy from the same release-note source.
(() => {
  const page = document.getElementById('changelogPage');
  document.getElementById('guideVersionBadge').textContent = 'v' + page.dataset.currentVersion;
  const guide = document.querySelector('.layout-frame');
  const active = new URLSearchParams(location.search).get('view') === 'changelog';
  page.hidden = !active;
  guide.hidden = active;
  document.querySelectorAll('[data-doc-view]').forEach(link => {
    if (link.dataset.docView === (active ? 'changelog' : 'guide')) link.setAttribute('aria-current', 'page');
  });
  if (active) {
    document.title = 'Zoltraak — Changelog';
    document.querySelector('.search-box').hidden = true;
    document.getElementById('themesLink').href = '?view=guide#themes-presets';
    // The existing guide theme handler is only relevant in guide view.
    document.getElementById('themesLink').addEventListener('click', event => { event.stopImmediatePropagation(); location.href = '?view=guide#themes-presets'; }, true);
  }
  const search = document.getElementById('changeSearch');
  const version = document.getElementById('changeVersion');
  const category = document.getElementById('changeCategory');
  const entries = [...page.querySelectorAll('.change-entry')];
  function filter() {
    const query = search.value.trim().toLowerCase();
    entries.forEach(entry => {entry.hidden = Boolean(category.value && entry.dataset.category !== category.value || query && !(entry.closest('.change-release').dataset.version+' '+entry.textContent).toLowerCase().includes(query));});
    page.querySelectorAll('.change-category').forEach(section => { section.hidden = ![...section.querySelectorAll('.change-entry')].some(entry=>!entry.hidden); });
    page.querySelectorAll('.change-release').forEach(release => {
      const markerMatches=release.dataset.superseded && !category.value && (!query || release.textContent.toLowerCase().includes(query));
      release.hidden = Boolean(version.value && release.dataset.version !== version.value || !markerMatches && ![...release.querySelectorAll('.change-entry')].some(entry=>!entry.hidden));
    });
    const count=entries.filter(entry=>!entry.hidden&&!entry.closest('.change-release').hidden).length;
    document.getElementById('changeCount').textContent=count+' matching '+(count===1?'change':'changes');
    document.getElementById('changeEmpty').hidden=[...page.querySelectorAll('.change-release')].some(release=>!release.hidden);
  }
  [search,version,category].forEach(control=>control.addEventListener('input',filter));
  document.getElementById('changeReset').addEventListener('click',()=>{search.value='';version.value='';category.value='';filter();search.focus();});
  document.getElementById('changeDetails').addEventListener('click',event=>{
    const expanded=event.currentTarget.getAttribute('aria-pressed')!=='true';
    entries.forEach(entry=>{entry.open=expanded;});
    event.currentTarget.setAttribute('aria-pressed',String(expanded));
    event.currentTarget.textContent=expanded?'Collapse details':'Expand details';
  });
  filter();
  if(active&&location.hash.startsWith('#release-')) document.getElementById(decodeURIComponent(location.hash.slice(1)))?.scrollIntoView();
  const allowed = ['panel','panel-face','panel-recess','panel-sunk','edge','edge-strong','ink','ink-dim','ink-faint','clear','caution','danger','route','brass','on-signal','lamp-off'];
  async function syncLiveTheme() {
    if (document.hidden || !/^https?:$/.test(location.protocol)) return;
    try {
      const response = await fetch(new URL('/guide/theme', location.href), {signal:AbortSignal.timeout(2000)});
      if (!response.ok) return;
      const theme = await response.json();
      if (!theme.variables || !['light','dark'].includes(theme.colorScheme)) return;
      allowed.forEach(name=>{
        const value=theme.variables['--'+name];
        if (/^#[a-f0-9]{6}$/i.test(value||'')) document.documentElement.style.setProperty('--c-'+name,value);
      });
      document.documentElement.setAttribute('data-theme',theme.colorScheme);
      const geometry = theme.geometry || {};
      if (['sharp','bevel','modern','rounded'].includes(geometry.radius)) document.documentElement.setAttribute('data-radius',geometry.radius);
      if (['compact','default','comfortable'].includes(geometry.density)) document.documentElement.setAttribute('data-density',geometry.density);
    } catch { /* Offline guide retains its current theme. */ }
  }
  void syncLiveTheme();
  setInterval(()=>void syncLiveTheme(),3000);
})();
