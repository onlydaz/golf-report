
import { Col, DatePicker, Form, Input, InputNumber, Row } from "antd";
import dayjs from "dayjs";

function GeneralInformationForm() {
  return (
    <Row gutter={18}>
      {/* Mã sản phẩm */}
      <Col span={8}>
        <Form.Item
          name="productCode"
          label="Mã sản phẩm"
          rules={[
            {
              required: true,
              whitespace: true,
              message: "Vui lòng nhập mã sản phẩm.",
            },
          ]}
        >
          <Input
            placeholder="Nhập mã sản phẩm"
            maxLength={100}
          />
        </Form.Item>
      </Col>

      {/* Số thứ tự gậy */}
      <Col span={8}>
        <Form.Item
          name="clubNumber"
          label="Số thứ tự gậy"
          rules={[
            {
              required: true,
              whitespace: true,
              message: "Vui lòng nhập số thứ tự gậy.",
            },
          ]}
        >
          <Input placeholder="Ví dụ: 01" />
        </Form.Item>
      </Col>

      {/* Thời gian đo */}
      <Col span={8}>
        <Form.Item
          name="measurementTime"
          label="Thời gian đo"
          rules={[
            {
              required: true,
              message: "Vui lòng chọn thời gian đo.",
            },
          ]}
        >
          <DatePicker
            showTime
            format="DD/MM/YYYY HH:mm:ss"
            disabledDate={(date) => date.isAfter(dayjs(), "day")}
            style={{ width: "100%" }}
          />
        </Form.Item>
      </Col>

      {/* Weight */}
      <Col span={8}>
        <Form.Item
          name="weight"
          label="Weight"
          rules={[
            {
              required: true,
              message: "Vui lòng nhập Weight.",
            },
            {
              type: "number",
              min: 0,
              message: "Weight phải là số không âm.",
            },
          ]}
        >
          <InputNumber
            min={0}
            step={0.01}
            style={{ width: "100%" }}
            placeholder="Nhập giá trị"
          />
        </Form.Item>
      </Col>
    </Row>
  );
}

export default GeneralInformationForm;
