pub fn render_html(username: &str, is_watch: bool) -> String {
  format!(
    r#"
    <!DOCTYPE html>
    <html lang="zh-CN">
      <head>
        <meta charset="UTF-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0, user-scalable=no" />
        <title>Quiz 接龙</title>
        <link rel="stylesheet" href="https://csstools.github.io/sanitize.css/13.0.0/sanitize.css" />
        <style>
          :root {{ --bg: #f4f4f4; --text: #333; --cell-size: 40px; }}
          body {{ font-family: monospace; background: var(--bg); color: var(--text); margin: 0; padding: 0; height: 100vh; display: flex; overflow: hidden; }}
          * {{ border-radius: 0 !important; box-sizing: border-box; }}
          .sidebar {{ width: 250px; background: #e0e0e0; border-right: 2px solid #000; display: flex; flex-direction: column; overflow-y: auto; flex-shrink: 0; }}
          .main {{ flex: 1; display: flex; flex-direction: column; overflow: hidden; }}
          .log-panel {{ width: 300px; background: #fff; border-left: 2px solid #000; overflow-y: auto; font-size: 12px; padding: 10px; flex-shrink: 0; }}
          @media (max-width: 800px) {{
            body {{ flex-direction: column; height: auto; overflow-y: auto; }}
            .sidebar {{ width: 100%; height: 180px; border-right: none; border-top: 2px solid #000; order: 2; }}
            .main {{ width: 100%; min-height: 60vh; order: 1; }}
            .log-panel {{ width: 100%; height: 200px; border-left: none; border-top: 2px solid #000; order: 3; }}
          }}
          .header {{ height: 60px; border-bottom: 2px solid #000; display: flex; align-items: center; justify-content: center; font-size: 1.1em; font-weight: bold; background: #fff; padding: 0 10px; }}
          .content {{ flex: 1; padding: 20px; overflow-y: auto; display: flex; justify-content: center; align-items: flex-start; }}
          .controls {{ min-height: 80px; height: auto; border-top: 2px solid #000; background: #ddd; display: flex; align-items: center; justify-content: center; gap: 10px; padding: 10px; flex-wrap: wrap; }}
          .grid {{ display: grid; grid-template-columns: repeat(auto-fill, var(--cell-size)); gap: 4px; width: 100%; max-width: 800px; padding: 10px; background: transparent; }}
          .cell {{
            width: var(--cell-size); height: var(--cell-size);
            background: #222; border: 1px solid #555;
            color: #000; display: flex; align-items: center; justify-content: center;
            font-size: 20px; font-weight: bold;
          }}
          .player-item {{ padding: 10px; border-bottom: 1px solid #999; display: flex; justify-content: space-between; align-items: center; font-size: 14px; }}
          .player-status {{ font-size: 10px; padding: 2px 4px; background: #333; color: #fff; }}
          button {{ padding: 12px 20px; border: 2px solid #000; background: #fff; cursor: pointer; font-weight: bold; font-size: 16px; min-width: 80px; }}
          button:hover {{ background: #eee; }}
          button:active {{ background: #ccc; }}
          input[type="text"] {{ padding: 10px; border: 2px solid #000; width: 100%; max-width: 300px; font-size: 16px; }}
          .active-turn {{ border-left: 6px solid red; background: #fff0f0; }}
          #result-area {{ display: none; padding: 20px; background: #fff; border: 2px solid #000; margin-top: 20px; width: 100%; max-width: 800px; }}
        </style>
      </head>
      <body>
        <div class="sidebar" id="player-list"></div>
        <div class="main">
          <div class="header" id="hint-box">...</div>
          <div class="content">
            <div style="width: 100%; display: flex; flex-direction: column; align-items: center;">
              <div class="grid" id="grid-box"></div>
              <div id="result-area"></div>
            </div>
          </div>
          <div class="controls" id="control-box">
            <div id="status-text">连接中...</div>
          </div>
        </div>
        <div class="log-panel" id="log-box"></div>
        <script>
          const isWatch = {};
          const username = "{}";
          let socket;
          let gameState = null;
          let timerInterval = null;

          function connect() {{
              const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
              const qs = isWatch ? '?spectate=true' : '';
              socket = new WebSocket(`${{protocol}}//${{window.location.host}}/ws${{qs}}`);
              socket.onopen = () => {{
                  document.getElementById('status-text').innerText = "已连接";
                  setInterval(() => socket.send(JSON.stringify({{ type: "Heartbeat", data: null }})), 5000);
                  if (!timerInterval) timerInterval = setInterval(updateUiTimer, 100);
              }};
              socket.onmessage = (event) => {{
                  const msg = JSON.parse(event.data);
                  if (msg.type === 'update') {{ gameState = msg.data; render(); }}
                  else if (msg.type === 'log') {{ const parts = msg.data.split(' [系统] '); log("系统", parts[1], parts[0]); }}
                  else if (msg.type === 'error') {{ alert(msg.data); }}
              }};
              socket.onclose = () => setTimeout(connect, 3000);
          }}

          function log(who, text, time) {{
              const box = document.getElementById('log-box');
              const div = document.createElement('div');
              div.style.marginBottom = "4px";
              div.style.wordBreak = "break-all";
              div.innerText = `${{time || new Date().toLocaleTimeString('en-GB')}} [${{who}}] ${{text}}`;
              box.appendChild(div);
              box.scrollTop = box.scrollHeight;
          }}

          function sendAction(act) {{ socket.send(JSON.stringify({{ type: "Action", data: {{ action: act }} }})); }}
          function sendAnswer() {{ socket.send(JSON.stringify({{ type: "Answer", data: {{ content: document.getElementById('ans-input').value }} }})); }}

          function updateUiTimer() {{
             if (!gameState || !gameState._localTargetTime) return;
             const rem = Math.max(0, (gameState._localTargetTime - Date.now()) / 1000);
             const els = document.querySelectorAll('.timer-text');
             els.forEach(el => el.innerText = rem.toFixed(1) + 's');
          }}

          function render() {{
              if (gameState.turn_deadline_ms) gameState._localTargetTime = Date.now() + gameState.turn_deadline_ms;
              else if (gameState.answer_deadline_ms) gameState._localTargetTime = Date.now() + gameState.answer_deadline_ms;
              else gameState._localTargetTime = null;

              const totalChars = gameState.grid.length;
              const hintText = gameState.phase === 'Settlement' ? "比赛结束" : (gameState.hint || "等待开始...");
              document.getElementById('hint-box').innerText = `${{hintText}} (共 ${{totalChars}} 字)`;

              const pList = document.getElementById('player-list');
              pList.innerHTML = '';
              gameState.players.forEach(p => {{
                  const div = document.createElement('div');
                  div.className = 'player-item';
                  div.style.backgroundColor = `hsl(${{p.color_hue}}, 70%, 90%)`;
                  if (p.status === 'Picking') div.classList.add('active-turn');
                  div.innerHTML = `<div><strong>${{p.id}}</strong> ${{p.is_me ? '(我)' : ''}}<br><small>字数: ${{p.obtained_count}}</small></div><div class="player-status">${{p.is_online?p.status:'OFFLINE'}}</div>`;
                  pList.appendChild(div);
              }});

              const grid = document.getElementById('grid-box');
              grid.innerHTML = '';
              gameState.grid.forEach(cell => {{
                  const div = document.createElement('div');
                  div.className = 'cell';
                  if (cell.owner_color_hue !== null) div.style.backgroundColor = `hsl(${{cell.owner_color_hue}}, 70%, 80%)`;
                  if (cell.char_content) div.innerText = cell.char_content;
                  grid.appendChild(div);
              }});

              const resArea = document.getElementById('result-area');
              if (gameState.phase === 'Settlement') {{
                  resArea.style.display = 'block';
                  let html = `<h3>正确答案：${{gameState.correct_answer}}</h3><h4>完整题面：</h4><p style="word-break:break-all">${{gameState.full_problem}}</p><h4>玩家回答：</h4><ul>`;
                  gameState.players.forEach(p => {{
                      let color = p.answer === gameState.correct_answer ? 'green' : 'red';
                      let showAns = p.answer === null ? '(未提交)' : (p.answer === '' ? '(空)' : p.answer);
                      html += `<li style="color:${{color}}">${{p.id}}: ${{showAns}}</li>`;
                  }});
                  resArea.innerHTML = html + '</ul>';
              }} else resArea.style.display = 'none';

              const ctrl = document.getElementById('control-box');
              if (isWatch) {{ ctrl.innerHTML = '<div>正在观战中...</div>'; return; }}
              const me = gameState.players.find(p => p.is_me);
              if (!me) {{ ctrl.innerHTML = ''; return; }}

              if (gameState.phase === 'Picking') {{
                  if (me.status === 'Picking') {{
                      ctrl.innerHTML = `<button onclick="sendAction('take')">要一个字 (<span class="timer-text">--</span>)</button>
                                        <button onclick="sendAction('stop')" style="background:#fdd">停止</button>`;
                  }} else ctrl.innerHTML = `<div>等待他人操作...</div>`;
              }} else if (gameState.phase === 'Answering') {{
                  if (me.status === 'Submitted') ctrl.innerHTML = `<div>答案已提交，等待其他人...</div>`;
                  else {{
                      const oldInput = document.getElementById('ans-input');
                      const draft = oldInput ? oldInput.value : (me.answer || '');
                      ctrl.innerHTML = `<input type="text" id="ans-input" placeholder="输入答案...">
                                        <button onclick="sendAnswer()">提交 (<span class="timer-text">--</span>)</button>`;
                      const newInput = document.getElementById('ans-input');
                      if (newInput) newInput.value = draft;
                  }}
              }} else if (gameState.phase === 'Settlement') ctrl.innerHTML = `<div>游戏结束</div>`;
              else ctrl.innerHTML = `<div>等待管理员 /start</div>`;

              updateUiTimer();
          }}
          connect();
        </script>
      </body>
    </html>
    "#,
    is_watch, username
  )
}
