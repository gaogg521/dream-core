<template>
  <!--
    表单页模板（全量 el-form 属性 + 校验 + 提交）
    技术栈：Vue 2 + Element UI + Options API + axios 封装
  -->
  <div class="form-page">
    <el-card shadow="never">
      <!-- 页头 -->
      <el-page-header @back="goBack" :content="pageTitle" slot="header"></el-page-header>

      <el-divider></el-divider>

      <!-- 表单 -->
      <el-form
        :model="form"
        :rules="rules"
        ref="form"
        label-width="120px"
        label-position="right"
        size="medium"
        :disabled="submitting"
      >
        <!-- 基本信息区 -->
        <el-divider content-position="left">基本信息</el-divider>

        <el-row :gutter="20">
          <el-col :span="12">
            <el-form-item label="名称" prop="name">
              <el-input v-model="form.name" placeholder="请输入名称" maxlength="50" show-word-limit clearable></el-input>
            </el-form-item>
          </el-col>
          <el-col :span="12">
            <el-form-item label="编号" prop="code">
              <el-input v-model="form.code" placeholder="请输入编号" clearable></el-input>
            </el-form-item>
          </el-col>
        </el-row>

        <el-row :gutter="20">
          <el-col :span="12">
            <el-form-item label="分类" prop="categoryId">
              <el-cascader
                v-model="form.categoryId"
                :options="categoryOptions"
                :props="{ value: 'id', label: 'name', children: 'children', checkStrictly: true }"
                placeholder="请选择分类"
                clearable
                filterable
                style="width: 100%"
              ></el-cascader>
            </el-form-item>
          </el-col>
          <el-col :span="12">
            <el-form-item label="标签" prop="tags">
              <el-select v-model="form.tags" multiple filterable allow-create placeholder="请选择或输入标签" style="width: 100%">
                <el-option v-for="tag in tagOptions" :key="tag.value" :label="tag.label" :value="tag.value"></el-option>
              </el-select>
            </el-form-item>
          </el-col>
        </el-row>

        <el-row :gutter="20">
          <el-col :span="12">
            <el-form-item label="状态" prop="status">
              <el-radio-group v-model="form.status">
                <el-radio :label="1">启用</el-radio>
                <el-radio :label="0">禁用</el-radio>
              </el-radio-group>
            </el-form-item>
          </el-col>
          <el-col :span="12">
            <el-form-item label="排序" prop="sort">
              <el-input-number v-model="form.sort" :min="0" :max="9999" :step="1"></el-input-number>
            </el-form-item>
          </el-col>
        </el-row>

        <!-- 时间信息区 -->
        <el-divider content-position="left">时间信息</el-divider>

        <el-row :gutter="20">
          <el-col :span="12">
            <el-form-item label="生效日期" prop="effectiveDate">
              <el-date-picker
                v-model="form.effectiveDate"
                type="daterange"
                range-separator="至"
                start-placeholder="开始日期"
                end-placeholder="结束日期"
                value-format="yyyy-MM-dd"
                clearable
                style="width: 100%"
              ></el-date-picker>
            </el-form-item>
          </el-col>
          <el-col :span="12">
            <el-form-item label="提醒时间" prop="remindTime">
              <el-date-picker
                v-model="form.remindTime"
                type="datetime"
                placeholder="选择提醒时间"
                value-format="yyyy-MM-dd HH:mm:ss"
                clearable
                style="width: 100%"
              ></el-date-picker>
            </el-form-item>
          </el-col>
        </el-row>

        <!-- 其他信息区 -->
        <el-divider content-position="left">其他信息</el-divider>

        <el-form-item label="附件" prop="fileList">
          <el-upload
            action="/api/upload"
            :on-success="handleUploadSuccess"
            :on-remove="handleUploadRemove"
            :before-upload="beforeUpload"
            :file-list="form.fileList"
            :limit="5"
            :on-exceed="handleExceed"
            list-type="text"
          >
            <el-button size="small" type="primary" icon="el-icon-upload">点击上传</el-button>
            <div slot="tip" class="el-upload__tip">支持上传 jpg/png/pdf 文件，单个文件不超过 10MB，最多 5 个</div>
          </el-upload>
        </el-form-item>

        <el-form-item label="描述" prop="description">
          <el-input
            type="textarea"
            v-model="form.description"
            :rows="4"
            maxlength="500"
            show-word-limit
            placeholder="请输入描述"
          ></el-input>
        </el-form-item>

        <el-form-item label="是否开启">
          <el-switch v-model="form.enabled" active-text="开启" inactive-text="关闭"></el-switch>
        </el-form-item>

        <!-- 按钮区 -->
        <el-form-item>
          <el-button type="primary" :loading="submitting" @click="handleSubmit">提 交</el-button>
          <el-button @click="handleReset">重 置</el-button>
          <el-button @click="goBack">返 回</el-button>
        </el-form-item>
      </el-form>
    </el-card>
  </div>
</template>

<script>
export default {
  name: 'FormPage',
  data() {
    return {
      pageTitle: '新增',
      submitting: false,
      // 表单数据
      form: {
        name: '',
        code: '',
        categoryId: [],
        tags: [],
        status: 1,
        sort: 0,
        effectiveDate: [],
        remindTime: '',
        fileList: [],
        description: '',
        enabled: true
      },
      // 下拉选项
      categoryOptions: [],
      tagOptions: [
        { label: '标签1', value: 'tag1' },
        { label: '标签2', value: 'tag2' },
        { label: '标签3', value: 'tag3' }
      ],
      // 校验规则
      rules: {
        name: [
          { required: true, message: '请输入名称', trigger: 'blur' },
          { min: 2, max: 50, message: '长度在 2 到 50 个字符', trigger: 'blur' }
        ],
        code: [
          { required: true, message: '请输入编号', trigger: 'blur' },
          { pattern: /^[A-Za-z0-9_-]+$/, message: '只能包含字母、数字、下划线和短横线', trigger: 'blur' }
        ],
        categoryId: [
          { required: true, message: '请选择分类', trigger: 'change' }
        ],
        status: [
          { required: true, message: '请选择状态', trigger: 'change' }
        ],
        effectiveDate: [
          { required: true, message: '请选择生效日期', trigger: 'change' }
        ]
      }
    }
  },
  created() {
    // 编辑模式时加载详情
    if (this.$route.params.id) {
      this.pageTitle = '编辑'
      this.loadDetail(this.$route.params.id)
    }
    this.loadCategoryOptions()
  },
  methods: {
    // 加载分类选项
    loadCategoryOptions() {
      this.$api.getCategoryTree().then(res => {
        this.categoryOptions = res.data
      })
    },
    // 加载详情
    loadDetail(id) {
      this.$api.getDetail({ id }).then(res => {
        this.form = Object.assign({}, this.form, res.data)
      })
    },
    // 上传前校验
    beforeUpload(file) {
      const isLt10M = file.size / 1024 / 1024 < 10
      if (!isLt10M) {
        this.$message.error('文件大小不能超过 10MB')
        return false
      }
      return true
    },
    // 上传成功
    handleUploadSuccess(response, file, fileList) {
      // 设置文件对象属性
      file.status = 'done'
      file.url = response.data.url
      file.fileId = response.data.fileId
      this.form.fileList = fileList
    },
    // 删除文件
    handleUploadRemove(file, fileList) {
      this.form.fileList = fileList
    },
    // 超出数量限制
    handleExceed() {
      this.$message.warning('最多上传 5 个文件')
    },
    // 提交表单
    handleSubmit() {
      this.$refs.form.validate(valid => {
        if (!valid) {
          this.$message.error('请完善表单信息')
          return false
        }
        this.submitting = true
        // 组装提交数据
        const submitData = Object.assign({}, this.form)
        // 处理级联选择器值（取最后一级）
        if (Array.isArray(submitData.categoryId) && submitData.categoryId.length > 0) {
          submitData.categoryId = submitData.categoryId[submitData.categoryId.length - 1]
        }
        // 处理日期范围
        if (Array.isArray(submitData.effectiveDate) && submitData.effectiveDate.length === 2) {
          submitData.effectiveStart = submitData.effectiveDate[0]
          submitData.effectiveEnd = submitData.effectiveDate[1]
          delete submitData.effectiveDate
        }
        this.$api.saveForm(submitData).then(() => {
          this.$message.success('提交成功')
          this.goBack()
        }).finally(() => {
          this.submitting = false
        })
      })
    },
    // 重置表单
    handleReset() {
      this.$refs.form.resetFields()
      this.form.fileList = []
    },
    // 返回
    goBack() {
      this.$router.go(-1)
    }
  }
}
</script>

<style>
.form-page {
  padding: 20px;
}
</style>
