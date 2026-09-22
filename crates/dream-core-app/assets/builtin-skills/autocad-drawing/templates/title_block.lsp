;;; title_block.lsp — 国标标题栏参数化绘制（AutoLISP）
;;; 用法：APPLOAD 加载本文件，命令行输入：
;;;   (GB-TITLE 0 0 180 56 "单位名称" "图名" "图号" "1:1" "Q235" "设计人")
;;; 参数：x0 y0 — 标题栏左下角；w h — 宽高；后接各字段文字。
;;; 说明：默认画 180x56 简化标题栏，含横向分栏与占位文字。

(defun GB-TITLE (x0 y0 w h unit title no scale mat designer / x1 x2 x3 x4 ymid)
  (setq x1 (+ x0 50.0)
        x2 (+ x0 90.0)
        x3 (+ x0 120.0)
        x4 (+ x0 150.0)
        ymid (+ y0 (/ h 2.0)))

  ;; 外框（粗实线）
  (command "._LAYER" "_S" "图框" "")
  (command "._RECTANG" (list x0 y0) (list (+ x0 w) (+ y0 h)))
  ;; 分栏（细实线）
  (command "._LAYER" "_S" "细实线" "")
  (command "._LINE" (list x1 y0) (list x1 (+ y0 h)) "")
  (command "._LINE" (list x2 y0) (list x2 (+ y0 h)) "")
  (command "._LINE" (list x3 y0) (list x3 (+ y0 h)) "")
  (command "._LINE" (list x4 y0) (list x4 (+ y0 h)) "")
  (command "._LINE" (list x0 ymid) (list (+ x0 w) ymid) "")
  (command "._LINE" (list x1 ymid) (list x1 (+ y0 h)) "")

  ;; 文字（粗体说明：文字样式须先建立 GB，见 setup_drawing.scr）
  (command "._LAYER" "_S" "文字" "")
  (command "._-STYLE" "GB" "" "")
  (GB-TXT x0 ymid unit 3.5)
  (GB-TXT x1 ymid title 5.0)
  (GB-TXT x1 y0 no 3.5)
  (GB-TXT x2 y0 scale 3.5)
  (GB-TXT x3 y0 mat 3.5)
  (GB-TXT x4 y0 designer 3.5)
  (princ)
)

;; 写单个文字（左下角定位）
(defun GB-TXT (x y str h)
  (if (and str (/= str ""))
    (command "._TEXT" (list (+ x 1.0) (+ y 1.0)) h "0" str))
  (princ)
)

(princ "\n国标标题栏已加载。用法：(GB-TITLE 0 0 180 56 \"单位\" \"图名\" \"图号\" \"1:1\" \"材料\" \"设计\")")
(princ)
