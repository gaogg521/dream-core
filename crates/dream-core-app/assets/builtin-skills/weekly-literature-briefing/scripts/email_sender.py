#!/usr/bin/env python3
"""
邮件发送模块
- 使用 SMTP 发送 HTML 邮件
- 支持附件
"""

import json
import smtplib
import os
from email.mime.multipart import MIMEMultipart
from email.mime.text import MIMEText
from email.mime.base import MIMEBase
from email import encoders


def load_config(config_path="config.json"):
    with open(config_path, "r", encoding="utf-8") as f:
        return json.load(f)


def send_email(subject, html_content, config_path="config.json", attachments=None):
    """
    发送 HTML 邮件
    
    Args:
        subject: 邮件主题
        html_content: HTML 内容
        config_path: 配置文件路径
        attachments: 附件路径列表
    """
    config = load_config(config_path)
    email_config = config.get("email", {})
    
    if not email_config.get("enabled", False):
        print("  [SKIP] 邮件发送已禁用")
        return {"success": False, "reason": "disabled"}
    
    smtp = email_config.get("smtp", {})
    recipient = email_config.get("recipient", "")
    sender_name = email_config.get("sender_name", config.get("profile", {}).get("name", "文献简报"))
    
    if not recipient or not smtp.get("user") or not smtp.get("auth_code"):
        print("  [ERROR] 邮件配置不完整，请检查 config.json 中的 email 部分")
        return {"success": False, "reason": "incomplete_config"}
    
    msg = MIMEMultipart("alternative")
    msg["Subject"] = subject
    msg["From"] = f"{sender_name} <{smtp['user']}>"
    msg["To"] = recipient
    
    html_part = MIMEText(html_content, "html", "utf-8")
    msg.attach(html_part)
    
    # 附件
    if attachments:
        for filepath in attachments:
            if os.path.exists(filepath):
                with open(filepath, "rb") as f:
                    part = MIMEBase("application", "octet-stream")
                    part.set_payload(f.read())
                    encoders.encode_base64(part)
                    filename = os.path.basename(filepath)
                    part.add_header("Content-Disposition", f"attachment; filename={filename}")
                    msg.attach(part)
    
    try:
        host = smtp.get("host", "smtp.example.com")
        port = smtp.get("port", 465)
        
        if port == 465:
            server = smtplib.SMTP_SSL(host, port, timeout=30)
        else:
            server = smtplib.SMTP(host, port, timeout=30)
            server.starttls()
        
        server.login(smtp["user"], smtp["auth_code"])
        server.sendmail(smtp["user"], recipient, msg.as_string())
        server.quit()
        
        print(f"  [OK] 邮件发送成功 → {recipient}")
        return {"success": True}
    except Exception as e:
        print(f"  [ERROR] 邮件发送失败: {e}")
        return {"success": False, "reason": str(e)}


if __name__ == "__main__":
    import sys
    if len(sys.argv) < 3:
        print("Usage: python email_sender.py <subject> <html_file> [config_path]")
        sys.exit(1)
    
    subject = sys.argv[1]
    html_file = sys.argv[2]
    config_path = sys.argv[3] if len(sys.argv) > 3 else "config.json"
    
    with open(html_file, "r", encoding="utf-8") as f:
        html = f.read()
    
    result = send_email(subject, html, config_path)
    print(json.dumps(result, ensure_ascii=False))
