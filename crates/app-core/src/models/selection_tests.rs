use super::lifecycle::resolve_new_conversation_selection;
use ipc_types::ModelSelection;

#[test]
fn installed_platform_preference_never_overwrites_an_explicit_choice() {
    let gguf = ModelSelection {
        model_id: "qwen/qwen3.8-27b".to_owned(),
        model_version: "gguf".to_owned(),
    };
    let mlx = ModelSelection {
        model_id: "qwen/qwen3.8-27b-mlx".to_owned(),
        model_version: "mlx".to_owned(),
    };
    let candidates = vec![(gguf.clone(), true), (mlx.clone(), true)];
    assert_eq!(
        resolve_new_conversation_selection(None, &candidates).as_ref(),
        Some(&gguf)
    );
    assert_eq!(
        resolve_new_conversation_selection(Some(&gguf), &candidates),
        Some(gguf.clone())
    );
    assert_eq!(
        resolve_new_conversation_selection(Some(&mlx), &candidates),
        Some(mlx.clone())
    );
    // Normal-mode candidates exclude MLX, even when its preference is saved.
    assert_eq!(
        resolve_new_conversation_selection(Some(&mlx), &[(gguf.clone(), true)]),
        Some(gguf.clone())
    );
    assert_eq!(
        resolve_new_conversation_selection(None, &[(gguf.clone(), true), (mlx, false)]),
        Some(gguf)
    );
}
