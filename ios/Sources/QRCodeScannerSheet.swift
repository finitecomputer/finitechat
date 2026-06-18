import AVFoundation
import SwiftUI

struct QRCodeScannerSheet: View {
    static var canUseCamera: Bool {
        !ProcessInfo.processInfo.isiOSAppOnMac && AVCaptureDevice.default(for: .video) != nil
    }

    let onScanned: (String) -> Void
    @Environment(\.dismiss) private var dismiss
    @State private var authStatus = AVCaptureDevice.authorizationStatus(for: .video)
    @State private var scannerNonce = UUID()

    var body: some View {
        NavigationStack {
            VStack(spacing: 12) {
                content
                    .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .overlay {
                        RoundedRectangle(cornerRadius: 12, style: .continuous)
                            .stroke(.secondary.opacity(0.25), lineWidth: 1)
                    }

                Spacer(minLength: 0)
            }
            .padding(16)
            .navigationTitle("Scan Code")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        dismiss()
                    }
                }
            }
            .onAppear {
                ensureCameraPermission()
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        switch authStatus {
        case .authorized:
            QRCodeScannerView { value in
                onScanned(value)
                dismiss()
            }
            .id(scannerNonce)
            .frame(maxWidth: .infinity)
            .aspectRatio(1, contentMode: .fit)

        case .notDetermined:
            ProgressView("Requesting camera permission...")
                .frame(maxWidth: .infinity, minHeight: 240)

        case .denied, .restricted:
            ContentUnavailableView(
                "Camera Unavailable",
                systemImage: "camera.fill",
                description: Text("Paste the invite or profile code instead.")
            )
            .frame(maxWidth: .infinity, minHeight: 240)

        @unknown default:
            ContentUnavailableView("Camera Unavailable", systemImage: "camera.fill")
                .frame(maxWidth: .infinity, minHeight: 240)
        }
    }

    private func ensureCameraPermission() {
        let status = AVCaptureDevice.authorizationStatus(for: .video)
        authStatus = status
        guard status == .notDetermined else { return }

        AVCaptureDevice.requestAccess(for: .video) { granted in
            DispatchQueue.main.async {
                authStatus = granted ? .authorized : .denied
                scannerNonce = UUID()
            }
        }
    }
}

private struct QRCodeScannerView: UIViewControllerRepresentable {
    let onCode: (String) -> Void

    func makeUIViewController(context: Context) -> QRCodeScannerViewController {
        let viewController = QRCodeScannerViewController()
        viewController.onCode = onCode
        return viewController
    }

    func updateUIViewController(_ uiViewController: QRCodeScannerViewController, context: Context) {}
}

private final class QRCodeScannerViewController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onCode: ((String) -> Void)?

    private let session = AVCaptureSession()
    private var previewLayer: AVCaptureVideoPreviewLayer?
    private var didEmit = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black

        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input)
        else {
            return
        }
        session.addInput(input)

        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: DispatchQueue.main)
        output.metadataObjectTypes = [.qr]

        let layer = AVCaptureVideoPreviewLayer(session: session)
        layer.videoGravity = .resizeAspectFill
        previewLayer = layer
        view.layer.addSublayer(layer)
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        previewLayer?.frame = view.bounds
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        didEmit = false
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.session.startRunning()
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.session.stopRunning()
        }
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !didEmit else { return }
        guard let object = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              object.type == .qr,
              let value = object.stringValue,
              !value.isEmpty
        else {
            return
        }
        didEmit = true
        onCode?(value)
    }
}
