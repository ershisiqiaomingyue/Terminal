import java.io.File
import shark.AndroidObjectInspectors
import shark.FilteringLeakingObjectFinder
import shark.HeapAnalysisFailure
import shark.HeapAnalysisSuccess
import shark.HeapAnalyzer
import shark.MetadataExtractor
import shark.ObjectInspectors
import shark.OnAnalysisProgressListener

private fun json(value: String) = buildString {
  append('"')
  value.forEach { character ->
    when (character) {
      '\\' -> append("\\\\")
      '"' -> append("\\\"")
      '\n' -> append("\\n")
      '\r' -> append("\\r")
      '\t' -> append("\\t")
      else -> if (character.code < 0x20) append("\\u%04x".format(character.code)) else append(character)
    }
  }
  append('"')
}

fun main(args: Array<String>) {
  if (args.size != 1) {
    System.err.println("usage: AndroidToolShark <standard-hprof>")
    return
  }
  val file = File(args[0])
  if (!file.isFile) {
    System.err.println("HPROF file does not exist")
    return
  }
  val finder = FilteringLeakingObjectFinder(AndroidObjectInspectors.appLeakingObjectFilters)
  val inspectors = ObjectInspectors.jdkDefaults + AndroidObjectInspectors.appDefaults
  val analysis = HeapAnalyzer(OnAnalysisProgressListener.NO_OP).analyze(
    file,
    finder,
    emptyList(),
    true,
    inspectors,
    MetadataExtractor.NO_OP,
    null,
  )
  when (analysis) {
    is HeapAnalysisSuccess -> {
      val applicationLeaks = analysis.applicationLeaks.take(20).joinToString(",") { leak ->
        "{\"signature\":${json(leak.signature)},\"description\":${json(leak.shortDescription)},\"retainedBytes\":${leak.totalRetainedHeapByteSize ?: -1},\"retainedObjects\":${leak.totalRetainedObjectCount ?: -1}}"
      }
      val libraryLeaks = analysis.libraryLeaks.take(20).joinToString(",") { leak ->
        "{\"signature\":${json(leak.signature)},\"description\":${json(leak.shortDescription)},\"retainedBytes\":${leak.totalRetainedHeapByteSize ?: -1},\"retainedObjects\":${leak.totalRetainedObjectCount ?: -1}}"
      }
      println("{\"status\":\"ok\",\"analysisDurationMillis\":${analysis.analysisDurationMillis},\"applicationLeakCount\":${analysis.applicationLeaks.size},\"libraryLeakCount\":${analysis.libraryLeaks.size},\"unreachableObjectCount\":${analysis.unreachableObjects.size},\"applicationLeaks\":[$applicationLeaks],\"libraryLeaks\":[$libraryLeaks]}")
    }
    is HeapAnalysisFailure -> {
      analysis.exception.printStackTrace(System.err)
      println("{\"status\":\"error\",\"message\":${json(analysis.exception.message ?: analysis.exception.javaClass.name)}}")
    }
    else -> println("{\"status\":\"error\",\"message\":\"Shark returned an unsupported analysis result.\"}")
  }
}
