# 发票查验返回字段参考

本文件描述 `invoice_verification` 工具的返回票种映射及各票种返回字段含义，供解析结果时查阅。请求参数与票种枚举见 SKILL.md 正文。

## 1. 返回票种映射

工具返回的 `result.fplx` 为票种编码，映射关系如下：

| 票种编码 | 票种名称 |
| --- | --- |
| `01` | 增值税专用发票 |
| `03` | 机动车销售统一发票 |
| `04` | 增值税普通发票 |
| `08` | 增值税专用发票（电子） |
| `0910` | 全电普通发票 |
| `0920` | 全电专用发票 |
| `0901` | 全电纸质专用发票 |
| `0904` | 全电纸质普通发票 |
| `0903` | 全电纸质机动车销售统一发票 |
| `0915` | 全电纸质二手车统一销售发票 |
| `8208` | 电子发票（通行费发票） |
| `10` | 增值税普通发票（电子） |
| `11` | 增值税普通发票（卷式） |
| `14` | 增值税普通发票（通行费） |
| `15` | 二手车销售统一发票 |
| `61` | 电子发票（航空运输电子客票行程单） |
| `83` | 电子发票（铁路电子客票） |
| `100` | 区块链发票 |

## 2. 各票种返回字段总览

根据返回的 `result.fplx` 编码，`invoiceMaster` 和 `invoiceDetailList[]` 包含的字段如下：

| 票种编码 | `invoiceMaster` 字段 | `invoiceDetailList[]` 字段 |
| --- | --- | --- |
| `01` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsbh`, `gfdzdh`, `gfyhzh`, `xfmc`, `xfsbh`, `xfdzdh`, `xfyhzh`, `je`, `se`, `jshj`, `bz` | `hwmc`, `ggxh`, `dw`, `sl`, `dj`, `je`, `slv`, `se` |
| `03` | `fpdm`, `fphm`, `kprq`, `zfbz`, `skph`, `ghdw`, `sfzhm`, `gfsbh`, `cllx`, `cpxh`, `cd`, `hgzs`, `cjfy`, `sjdh`, `fdjhm`, `cjhm`, `jkzmsh`, `xhdwmc`, `dh`, `nsrsbh`, `zh`, `dz`, `khyh`, `zzssl`, `zzsse`, `swjgDm`, `jshj`, `wspzhm`, `dw`, `xcrs`, `swjgMc`, `dkbz`, `tszcbs`, `sjsl`, `sjse` | 无 |
| `04` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsbh`, `gfdzdh`, `gfyhzh`, `xfmc`, `xfsbh`, `xfdzdh`, `xfyhzh`, `jym`, `je`, `se`, `jshj`, `bz` | `hwmc`, `ggxh`, `dw`, `sl`, `dj`, `je`, `slv`, `se`, `sjsl`, `sjse` |
| `08` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsbh`, `gfdzdh`, `gfyhzh`, `xfmc`, `xfsbh`, `xfdzdh`, `xfyhzh`, `je`, `se`, `jshj`, `bz`, `ourl`, `purl` | `hwmc`, `ggxh`, `dw`, `sl`, `dj`, `je`, `slv`, `se` |
| `0910` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsh`, `gfdzdh`, `gfyhzh`, `xfmc`, `xfsh`, `xfdzdh`, `xfyhzh`, `jym`, `je`, `se`, `jshj`, `bz`, `ourl`, `purl` | `hwmc`, `ggxh`, `dw`, `sl`, `dj`, `je`, `slv`, `se`, `sjsl`, `sjse` |
| `0920` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsbh`, `gfdzdh`, `gfyhzh`, `xfmc`, `xfsbh`, `xfdzdh`, `xfyhzh`, `je`, `se`, `jshj`, `bz` | `hwmc`, `ggxh`, `dw`, `sl`, `dj`, `je`, `slv`, `se` |
| `0901` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsbh`, `gfdzdh`, `gfyhzh`, `xfmc`, `xfsbh`, `xfdzdh`, `xfyhzh`, `je`, `se`, `jshj`, `bz` | `hwmc`, `ggxh`, `dw`, `sl`, `dj`, `je`, `slv`, `se` |
| `0904` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsbh`, `gfdzdh`, `gfyhzh`, `xfmc`, `xfsbh`, `xfdzdh`, `xfyhzh`, `jym`, `je`, `se`, `jshj`, `bz` | `hwmc`, `ggxh`, `dw`, `sl`, `dj`, `je`, `slv`, `se`, `sjsl`, `sjse` |
| `0903` | 同 `03` | 无 |
| `0915` | 同 `15` | 无 |
| `8208` | `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsh`, `gfdzdh`, `gfyhzh`, `xfmc`, `xfsh`, `xfdzdh`, `xfyhzh`, `jym`, `je`, `se`, `jshj`, `bz` | `hwmc`, `cph`, `lx`, `txrqq`, `txrqz`, `je`, `slv`, `se`, `sjsl`, `sjse` |
| `10` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsh`, `gfdzdh`, `gfyhzh`, `xfmc`, `xfsh`, `xfdzdh`, `xfyhzh`, `jym`, `je`, `se`, `jshj`, `bz`, `ourl`, `purl` | `hwmc`, `ggxh`, `dw`, `sl`, `dj`, `je`, `slv`, `se`, `sjsl`, `sjse` |
| `11` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsh`, `xfmc`, `xfsh`, `jym`, `je`, `se`, `jshj`, `bz` | `xm`, `sl`, `hsdj`, `hsje` |
| `14` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsh`, `gfdzdh`, `gfyhzh`, `xfmc`, `xfsh`, `xfdzdh`, `xfyhzh`, `jym`, `je`, `se`, `jshj`, `bz` | `hwmc`, `cph`, `lx`, `txrqq`, `txrqz`, `je`, `slv`, `se`, `sjsl`, `sjse` |
| `15` | `fpdm`, `fphm`, `kprq`, `zfbz`, `skph`, `gfdw`, `gfhm`, `gfdz`, `gfdh`, `cpzh`, `djzh`, `cllx`, `cjhj`, `cjhm`, `cpxh`, `cgsmc`, `xfdw`, `xfhm`, `xfdz`, `xfdh`, `jydw`, `jydz`, `jysbh`, `jyyhzh`, `jydh`, `scmc`, `scsbh`, `scdz`, `scyhzh`, `scdh`, `bz` | 无 |
| `61` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `gfsbh`, `fplx`, `jshj`, `hjse`, `gngjbz`, `gpdh`, `lkxm`, `sfzjhm`, `dzkphm`, `hdxh`, `sfz`, `mdz`, `cyr`, `hbh`, `zwdj`, `cyrq`, `qfsj`, `kpjb` | 无 |
| `83` | `fpdm`, `fphm`, `kprq`, `zfbz`, `gfmc`, `pz`, `jshjcn`, `se`, `ywlx`, `cfsj`, `kttz`, `cc`, `gfsbh`, `fplx`, `xb`, `ccrq`, `cx`, `cfz`, `dzkph`, `name`, `zjh`, `jshj`, `je`, `ddz`, `slv`, `xw` | 无 |
| `100` | `fpdm`, `fphm`, `kprq`, `gfmc`, `xfmc`, `xfsbh`, `jym`, `je`, `se`, `jshj` | 无 |

## 3. 字段含义

### 3.1 通用主信息字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `fpdm` | `String` | 发票代码 |
| `fphm` | `String` | 发票号码 |
| `kprq` | `String` | 开票日期 |
| `zfbz` | `String` | 发票状态/作废标志。`0` 正常，`2` 作废，`3` 红冲 |
| `cycs` | `String` | 查验次数 |
| `cysj` | `String` | 查验时间 |

### 3.2 购销方与金额字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `gfmc` | `String` | 购方名称 |
| `gfsbh` | `String` | 购方纳税人识别号 |
| `gfsh` | `String` | 购方纳税人识别号 |
| `gfdzdh` | `String` | 购方地址、电话 |
| `gfyhzh` | `String` | 购方开户行及账号 |
| `xfmc` | `String` | 销方名称 |
| `xfsbh` | `String` | 销方纳税人识别号 |
| `xfsh` | `String` | 销方纳税人识别号 |
| `xfdzdh` | `String` | 销方地址、电话 |
| `xfyhzh` | `String` | 销方开户行及账号 |
| `jym` | `String` | 校验码 |
| `je` | `BigDecimal` | 金额合计 |
| `se` | `BigDecimal` | 税额合计 |
| `jshj` | `BigDecimal` | 价税合计 |
| `bz` | `String` | 备注 |
| `jqbh` | `String` | 机器编号 |
| `sbbh` | `String` | 设备编号 |
| `cpybz` | `String` | 成品油标志 |
| `qdbz` | `String` | 清单标志 |
| `purl` | `String` | PDF 版式文件下载链接 |
| `ourl` | `String` | OFD 版式文件下载链接 |
| `txfbz` | `String` | 通行费标志 |
| `shy` | `String` | 收货员 |

### 3.3 明细字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `hwmc` | `String` | 货物或应税劳务名称 |
| `ggxh` | `String` | 规格型号 |
| `dw` | `String` | 单位 |
| `sl` | `String`/`BigDecimal` | 数量 |
| `dj` | `BigDecimal` | 单价 |
| `je` | `BigDecimal` | 金额 |
| `slv` | `BigDecimal` | 税率 |
| `se` | `BigDecimal` | 税额 |
| `tszcbs` | `String` | 特殊政策标识 |
| `sjsl` | `BigDecimal` | 实际税率 |
| `sjse` | `BigDecimal` | 实际税额 |
| `xm` | `String` | 项目 |
| `hsdj` | `BigDecimal` | 含税单价 |
| `hsje` | `BigDecimal` | 含税金额 |
| `cph` | `String` | 车牌号 |
| `lx` | `String` | 类型 |
| `txrqq` | `String` | 通行日期起 |
| `txrqz` | `String` | 通行日期止 |

### 3.4 机动车专用字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `skph` | `String` | 机器编码 |
| `ghdw` | `String` | 购方名称 |
| `sfzhm` | `String` | 身份证号码 |
| `cllx` | `String` | 车辆类型 |
| `cpxh` | `String` | 厂牌型号 |
| `cd` | `String` | 产地 |
| `hgzs` | `String` | 合格证号 |
| `cjfy` | `BigDecimal` | 不含税价 |
| `sjdh` | `String` | 商检单号 |
| `fdjhm` | `String` | 发动机号 |
| `cjhm` | `String` | 车架号码 |
| `jkzmsh` | `String` | 进口证明书号 |
| `xhdwmc` | `String` | 销货单位名称 |
| `dh` | `String` | 电话 |
| `nsrsbh` | `String` | 销方纳税人识别号 |
| `zh` | `String` | 账号 |
| `dz` | `String` | 地址 |
| `khyh` | `String` | 开户银行 |
| `zzssl` | `BigDecimal` | 增值税税率 |
| `zzsse` | `BigDecimal` | 增值税税额 |
| `swjgDm` | `String` | 主管税务机关代码 |
| `wspzhm` | `String` | 完税凭证号码 |
| `xcrs` | `String` | 限乘人数 |
| `swjgMc` | `String` | 主管税务机关名称 |
| `dkbz` | `String` | 代开标志 |

### 3.5 二手车专用字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `gfdw` | `String` | 买方单位/个人 |
| `gfhm` | `String` | 买方单位代码/身份证号 |
| `gfdz` | `String` | 买方单位/个人住址 |
| `gfdh` | `String` | 买方电话 |
| `cpzh` | `String` | 车牌照号 |
| `djzh` | `String` | 登记证号 |
| `cjhj` | `String` | 车价合计 |
| `cgsmc` | `String` | 转入地车辆车管所名称 |
| `xfdw` | `String` | 卖方单位/个人 |
| `xfhm` | `String` | 卖方单位代码/身份证号 |
| `xfdz` | `String` | 卖方单位/个人住址 |
| `xfdh` | `String` | 卖方电话 |
| `jydw` | `String` | 经营、拍卖单位 |
| `jydz` | `String` | 经营、拍卖单位地址 |
| `jysbh` | `String` | 经营、拍卖单位纳税人识别号 |
| `jyyhzh` | `String` | 开户银行及账号 |
| `jydh` | `String` | 经营、拍卖单位电话 |
| `scmc` | `String` | 二手车市场 |
| `scsbh` | `String` | 二手车市场纳税人识别号 |
| `scdz` | `String` | 二手车市场地址 |
| `scyhzh` | `String` | 二手车市场开户银行及账号 |
| `scdh` | `String` | 二手车市场电话 |

### 3.6 客运票专用字段

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `fplx` | `String` | 发票类型 |
| `fpztDm` | `String` | 发票状态代码 |
| `hjse` | `BigDecimal` | 合计税额 |
| `gngjbz` | `String` | 国内国际标识 |
| `gpdh` | `String` | GP 单 |
| `lkxm` | `String` | 旅客姓名 |
| `sfzjhm` | `String` | 有效身份证件号码 |
| `dzkphm` | `String` | 电子客票号码 |
| `hdxh` | `String` | 航段序号 |
| `sfz` | `String` | 始发站 |
| `mdz` | `String` | 目的站 |
| `cyr` | `String` | 承运人 |
| `hbh` | `String` | 航班号 |
| `zwdj` | `String` | 座位等级 |
| `cyrq` | `String` | 承运日期 |
| `qfsj` | `String` | 起飞时间 |
| `kpjb` | `String` | 客票级别/客票类别 |
| `pz` | `String` | 票种 |
| `jshjcn` | `String` | 价税合计中文大写 |
| `ywlx` | `String` | 业务类型，`0` 退，`1` 售 |
| `cfsj` | `String` | 出发时间 |
| `kttz` | `String` | 空调特征 |
| `cc` | `String` | 车次 |
| `xb` | `String` | 席别 |
| `ccrq` | `String` | 乘车日期 |
| `cx` | `String` | 车厢 |
| `cfz` | `String` | 出发站 |
| `dzkph` | `String` | 电子客票号 |
| `name` | `String` | 姓名 |
| `zjh` | `String` | 证件号 |
| `ddz` | `String` | 到达站 |
| `xw` | `String` | 席位 |
