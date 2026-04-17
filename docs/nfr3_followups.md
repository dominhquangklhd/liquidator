# Ghi chú cải thiện NFR3

## Đánh giá hiện tại

- Hệ thống đã có các thành phần hỗ trợ mở rộng: bộ đệm nóng, lưu trữ lạnh SQLite, chỉ mục ngược asset -> users, luồng xử lý sự kiện, đồng bộ nền.
- Tuy nhiên, hiện mới dừng ở mức nền tảng kiến trúc, chưa đủ để khẳng định đạt chỉ tiêu quy mô lớn (khoảng 10^6 người dùng và bộ nhớ dưới 2GB).

## Điểm cần cải thiện

- Tăng quy mô bộ đệm nóng từ mặc định 100 mục lên một tỷ lệ động theo tổng số người dùng, thay vì cố định quá nhỏ.
- Tối ưu cấu trúc chỉ mục ngược: tránh dùng danh sách tuyến tính cho các tập người dùng lớn.
- Bổ sung cơ chế xử lý theo đợt để gom sự kiện trong một khoảng thời gian ngắn trước khi tính lại rủi ro.
- Giảm số lượng đối tượng phải đánh giá lợi nhuận bằng chiến lược ưu tiên top-K.
- Kiểm chứng bộ nhớ thực tế bằng benchmark thay vì chỉ dựa trên thiết kế.

## Hướng triển khai sau này

- Đo độ trễ và mức sử dụng bộ nhớ trên dữ liệu giả lập lớn.
- Thiết kế lại chỉ mục người dùng theo tài sản để tra cứu nhanh hơn.
- Tách rõ phần tính toán rủi ro, xếp hạng cơ hội và lưu trữ để dễ tối ưu từng lớp.