"""checks 包：每个 check_*.py 是一个独立检查器。"""
from .check_margin import run as margin_run
from .check_date import run as date_run
from .check_page_grid import run as page_grid_run
from .check_body import run as body_run
from .check_header import run as header_run
from .check_banji import run as banji_run
from .check_seal import run as seal_run
from .check_pagination import run as pagination_run
from .check_attachment import run as attachment_run

CHECKS = [
    (margin_run, "margin", "版心/页边距"),
    (page_grid_run, "page-grid", "页面网格"),
    (header_run, "header", "版头"),
    (body_run, "body", "主体"),
    (banji_run, "banji", "版记"),
    (seal_run, "seal", "落款印章"),
    (pagination_run, "pagination", "页码分页"),
    (date_run, "date-number", "日期双轨"),
    (attachment_run, "attachment", "附件三态"),
]

__all__ = ["CHECKS"]