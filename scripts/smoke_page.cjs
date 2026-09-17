// Browser regression checks with a deterministic AstrBot bridge fixture.
// Run with Playwright available in NODE_PATH: node scripts/smoke_page.cjs
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');

(async () => {
  const browser = await chromium.launch({headless:true, ...(process.env.PLAYWRIGHT_CHANNEL ? {channel:process.env.PLAYWRIGHT_CHANNEL} : {})});
  try {
    const page = await browser.newPage({viewport:{width:1100,height:850}});
    const root = path.resolve(__dirname,'../pages/memory');
    await page.route('http://mnemo.test/**', async route => {
      const name = new URL(route.request().url()).pathname.slice(1) || 'index.html';
      assert(['index.html','app.js','style.css'].includes(name));
      await route.fulfill({body:fs.readFileSync(path.join(root,name)),contentType:name.endsWith('.js')?'text/javascript':name.endsWith('.css')?'text/css':'text/html'});
    });
    await page.addInitScript(() => {
      window.fixtureFail = false;
      window.AstrBotPluginPage = {
        ready: async () => ({isDark:false}), onContext: () => {},
        apiGet: async (endpoint, query) => {
          if (window.fixtureFail) throw new Error('模拟读取失败');
          if (endpoint.endsWith('status')) return {enabled:true,available:true,inspector_available:true,capture_enabled:true,diary_enabled:true,recall_enabled:true,capture_errors:0,database_path:'/data/mnemokernel.sqlite3'};
          if (query.collection === 'scopes') return {items:[{scope_id:'a'.repeat(64),platform_id:'测试平台',session_id:'12345',persona_id:'默认人格',state:'default',conversation_kind:'private'}],total:1,offset:0};
          return {items:query.query ? [] : [{id:'test',kind:'fact',state:'active',time_ms:1,content:'<img src=x onerror="window.injected=true">这是测试记忆'}],total:21,offset:query.offset};
        }
      };
    });
    await page.goto('http://mnemo.test/');
    await page.waitForSelector('article');
    assert.equal(await page.locator('article img').count(),0);
    assert.equal(await page.evaluate(() => window.injected),undefined);
    await page.locator('#next').click();
    await page.waitForFunction(() => document.querySelector('#count').textContent.includes('第 2 页'));
    await page.locator('#query').fill('没有这条记录');
    await page.locator('#search-form button').click();
    await page.waitForFunction(() => document.querySelector('#notice').textContent.includes('没有匹配记录'));
    assert.equal(await page.locator('article').count(),0);
    await page.locator('#query').fill('');
    await page.locator('#search-form button').click();
    await page.waitForSelector('article');
    await page.setViewportSize({width:390,height:844});
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    fs.mkdirSync(path.resolve(__dirname,'../dist/page-preview'),{recursive:true});
    await page.screenshot({path:path.resolve(__dirname,'../dist/page-preview/mobile.png'),fullPage:true});
    await page.evaluate(() => { window.fixtureFail = true; });
    await page.locator('#refresh').click();
    await page.waitForFunction(() => document.querySelector('#notice').textContent.includes('模拟读取失败'));
    assert.equal(await page.locator('article').count(),0);
    console.log('page smoke: pass (render, escaped text, pagination, search, failure clearing, mobile layout)');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode=1; });
