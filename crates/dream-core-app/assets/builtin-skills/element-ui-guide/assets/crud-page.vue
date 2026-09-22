<template>
  <!--
    CRUD 列表页模板（Avue 布局 + 原生 Element 实现）
    布局结构：搜索栏 + 操作按钮 + 表格 + 分页 + 弹窗表单
    技术栈：Vue 2 + Element UI + Options API + axios 封装
    后端分页返回格式：{ records, total, current, size }
  -->
  <div class="crud-page">
    <!-- ========== 搜索栏 ========== -->
    <el-card shadow="never" class="search-card">
      <el-form :inline="true" :model="searchForm" class="search-form" ref="searchForm">
        <el-form-item label="名称" prop="name">
          <el-input v-model="searchForm.name" placeholder="请输入名称" clearable @keyup.enter.native="handleSearch"></el-input>
        </el-form-item>
        <el-form-item label="状态" prop="status">
          <el-select v-model="searchForm.status" placeholder="请选择状态" clearable>
            <el-option label="启用" :value="1"></el-option>
            <el-option label="禁用" :value="0"></el-option>
          </el-select>
        </el-form-item>
        <el-form-item label="日期" prop="dateRange">
          <el-date-picker
            v-model="searchForm.dateRange"
            type="daterange"
            range-separator="至"
            start-placeholder="开始日期"
            end-placeholder="结束日期"
            value-format="yyyy-MM-dd"
            clearable
          ></el-date-picker>
        </el-form-item>
        <el-form-item>
          <el-button type="primary" icon="el-icon-search" @click="handleSearch">搜索</el-button>
          <el-button icon="el-icon-refresh-left" @click="handleReset">重置</el-button>
        </el-form-item>
      </el-form>
    </el-card>

    <!-- ========== 表格区域 ========== -->
    <el-card shadow="never" class="table-card">
      <!-- 操作按钮栏 -->
      <div class="toolbar">
        <el-button type="primary" icon="el-icon-plus" @click="handleAdd">新增</el-button>
        <el-button type="danger" icon="el-icon-delete" :disabled="selection.length === 0" @click="handleBatchDelete">批量删除</el-button>
        <el-button icon="el-icon-download" @click="handleExport">导出</el-button>
        <el-button icon="el-icon-refresh" circle @click="loadData" style="float: right"></el-button>
      </div>

      <!-- 数据表格 -->
      <el-table
        :data="tableData"
        border
        v-loading="loading"
        @selection-change="handleSelectionChange"
        style="width: 100%"
      >
        <el-table-column type="selection" width="55" align="center"></el-table-column>
        <el-table-column type="index" label="序号" width="50" align="center"></el-table-column>
        <el-table-column prop="name" label="名称" min-width="120" show-overflow-tooltip></el-table-column>
        <el-table-column prop="status" label="状态" width="100" align="center">
          <template slot-scope="scope">
            <el-tag :type="scope.row.status === 1 ? 'success' : 'info'" size="small">
              {{ scope.row.status === 1 ? '启用' : '禁用' }}
            </el-tag>
          </template>
        </el-table-column>
        <el-table-column prop="createTime" label="创建时间" width="180" align="center"></el-table-column>
        <el-table-column prop="remark" label="备注" min-width="200" show-overflow-tooltip></el-table-column>
        <el-table-column label="操作" width="180" align="center" fixed="right">
          <template slot-scope="scope">
            <el-button size="mini" type="text" @click="handleEdit(scope.row)">编辑</el-button>
            <el-button size="mini" type="text" style="color: #F56C6C" @click="handleDelete(scope.row)">删除</el-button>
          </template>
        </el-table-column>
      </el-table>

      <!-- 分页 -->
      <el-pagination
        class="pagination"
        @size-change="handleSizeChange"
        @current-change="handleCurrentChange"
        :current-page="pagination.current"
        :page-sizes="[10, 20, 50, 100]"
        :page-size="pagination.size"
        :total="pagination.total"
        layout="total, sizes, prev, pager, next, jumper"
        background
      ></el-pagination>
    </el-card>

    <!-- ========== 新增/编辑弹窗 ========== -->
    <el-dialog
      :title="dialogType === 'add' ? '新增' : '编辑'"
      :visible.sync="dialogVisible"
      width="600px"
      :before-close="handleDialogClose"
    >
      <el-form :model="form" :rules="rules" ref="form" label-width="100px">
        <el-form-item label="名称" prop="name">
          <el-input v-model="form.name" placeholder="请输入名称"></el-input>
        </el-form-item>
        <el-form-item label="状态" prop="status">
          <el-radio-group v-model="form.status">
            <el-radio :label="1">启用</el-radio>
            <el-radio :label="0">禁用</el-radio>
          </el-radio-group>
        </el-form-item>
        <el-form-item label="备注" prop="remark">
          <el-input type="textarea" v-model="form.remark" :rows="3" placeholder="请输入备注"></el-input>
        </el-form-item>
      </el-form>
      <span slot="footer">
        <el-button @click="dialogVisible = false">取 消</el-button>
        <el-button type="primary" :loading="submitting" @click="handleSubmit">确 定</el-button>
      </span>
    </el-dialog>
  </div>
</template>

<script>
export default {
  name: 'CrudPage',
  data() {
    return {
      // 搜索表单
      searchForm: {
        name: '',
        status: '',
        dateRange: []
      },
      // 表格数据
      tableData: [],
      loading: false,
      selection: [],
      // 分页参数
      pagination: {
        current: 1,
        size: 10,
        total: 0
      },
      // 弹窗
      dialogVisible: false,
      dialogType: 'add', // add | edit
      submitting: false,
      // 表单数据
      form: {
        id: '',
        name: '',
        status: 1,
        remark: ''
      },
      // 表单校验规则
      rules: {
        name: [
          { required: true, message: '请输入名称', trigger: 'blur' },
          { min: 2, max: 20, message: '长度在 2 到 20 个字符', trigger: 'blur' }
        ],
        status: [
          { required: true, message: '请选择状态', trigger: 'change' }
        ]
      }
    }
  },
  created() {
    this.loadData()
  },
  methods: {
    // 加载列表数据
    loadData() {
      this.loading = true
      // 组装查询参数
      const params = {
        current: this.pagination.current,
        size: this.pagination.size,
        name: this.searchForm.name,
        status: this.searchForm.status,
        startTime: this.searchForm.dateRange && this.searchForm.dateRange[0] ? this.searchForm.dateRange[0] : '',
        endTime: this.searchForm.dateRange && this.searchForm.dateRange[1] ? this.searchForm.dateRange[1] : ''
      }
      // axios 封装调用
      this.$api.getPageList(params).then(res => {
        // 后端返回格式：{ records, total, current, size }
        this.tableData = res.data.records
        this.pagination.total = res.data.total
      }).catch(() => {
        this.$message.error('加载数据失败')
      }).finally(() => {
        this.loading = false
      })
    },
    // 搜索
    handleSearch() {
      this.pagination.current = 1
      this.loadData()
    },
    // 重置搜索
    handleReset() {
      this.$refs.searchForm.resetFields()
      this.handleSearch()
    },
    // 选择变化
    handleSelectionChange(val) {
      this.selection = val
    },
    // 每页条数变化
    handleSizeChange(val) {
      this.pagination.size = val
      this.pagination.current = 1
      this.loadData()
    },
    // 当前页变化
    handleCurrentChange(val) {
      this.pagination.current = val
      this.loadData()
    },
    // 新增
    handleAdd() {
      this.dialogType = 'add'
      this.form = { id: '', name: '', status: 1, remark: '' }
      this.dialogVisible = true
    },
    // 编辑
    handleEdit(row) {
      this.dialogType = 'edit'
      this.form = Object.assign({}, row)
      this.dialogVisible = true
    },
    // 删除
    handleDelete(row) {
      this.$confirm('确认删除该记录?', '提示', {
        confirmButtonText: '确定',
        cancelButtonText: '取消',
        type: 'warning'
      }).then(() => {
        this.$api.delete({ id: row.id }).then(() => {
          this.$message.success('删除成功')
          this.loadData()
        })
      }).catch(() => {})
    },
    // 批量删除
    handleBatchDelete() {
      const ids = this.selection.map(item => item.id)
      this.$confirm(`确认删除选中的 ${ids.length} 条记录?`, '提示', {
        confirmButtonText: '确定',
        cancelButtonText: '取消',
        type: 'warning'
      }).then(() => {
        this.$api.batchDelete({ ids }).then(() => {
          this.$message.success('批量删除成功')
          this.loadData()
        })
      }).catch(() => {})
    },
    // 导出
    handleExport() {
      const params = {
        name: this.searchForm.name,
        status: this.searchForm.status
      }
      this.$api.exportData(params).then(res => {
        // 处理文件下载
        const blob = new Blob([res.data])
        const url = window.URL.createObjectURL(blob)
        const link = document.createElement('a')
        link.href = url
        link.download = '导出数据.xlsx'
        link.click()
        window.URL.revokeObjectURL(url)
      })
    },
    // 提交表单
    handleSubmit() {
      this.$refs.form.validate(valid => {
        if (!valid) {
          this.$message.error('请完善表单信息')
          return false
        }
        this.submitting = true
        const apiName = this.dialogType === 'add' ? 'add' : 'update'
        this.$api[apiName](this.form).then(() => {
          this.$message.success(this.dialogType === 'add' ? '新增成功' : '编辑成功')
          this.dialogVisible = false
          this.loadData()
        }).finally(() => {
          this.submitting = false
        })
      })
    },
    // 弹窗关闭
    handleDialogClose(done) {
      this.$refs.form.resetFields()
      if (done) done()
    }
  }
}
</script>

<style>
.crud-page {
  padding: 20px;
}
.search-card {
  margin-bottom: 20px;
}
.search-card .el-form-item {
  margin-bottom: 0;
}
.table-card .toolbar {
  margin-bottom: 15px;
}
.pagination {
  margin-top: 20px;
  text-align: right;
}
</style>
